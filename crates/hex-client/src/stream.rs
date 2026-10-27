//! Hex's live transcription streams (`/streams`, Hex's `docs/streams.md`): the app records the
//! microphone itself and pushes 16 kHz mono samples; Hex transcribes them on the model its
//! dictation has loaded and sends partial text while it listens, one segment per window (cut at
//! pauses), and the final text. Hex's own dictation always goes first, so a stream's text can lag
//! while the user dictates. One stream runs at a time, and Hex ends one that gets neither audio
//! nor a heartbeat for 10 s.

use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Hex, NOT_RUNNING};

/// The rate every stream's samples are in.
pub const STREAM_RATE: u32 = 16_000;

pub const STREAM_BUSY: &str = "Hex is already transcribing a stream";
pub const NO_MODEL: &str = "Hex has no dictation model loaded";
pub const NO_ENGINE: &str = "Hex's transcription engine isn't running";
/// Hex's code for audio it can't take yet: it has 120 s it hasn't transcribed. Push it again later.
pub const BACKLOG_FULL: &str = "stream-backlog-full";

/// Hex's largest audio request is 256 KiB, so longer pushes go in pieces.
const MAX_PUSH_SAMPLES: usize = 256 * 1024 / 4;

#[derive(Clone, Debug)]
pub struct Stream {
    pub id: u64,
    owner: String,
    /// The model and language Hex transcribes with: dictation's.
    pub model: String,
    pub language: String,
    /// Hex sends `Partial`s: its model streams. Otherwise only segments and the final.
    pub partials: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub segment: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Final {
    pub text: String,
    pub segments: Vec<Segment>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    /// The window being heard: `stable` won't change, `tentative` may. Replaces the last partial.
    Partial {
        segment: u64,
        stable: String,
        tentative: String,
    },
    /// A finished window.
    Segment(Segment),
    /// Every segment; the stream has ended.
    Final(Final),
    /// Hex ended the stream: `lease-expired`, `model-changed`, `transcription-failed`, `hex-quitting`.
    Error {
        code: String,
        message: String,
    },
    Cancelled,
}

impl StreamEvent {
    fn parse(v: &Value) -> Option<StreamEvent> {
        let text = |k: &str| v[k].as_str().unwrap_or("").to_owned();
        Some(match v["type"].as_str()? {
            "partial" => StreamEvent::Partial {
                segment: v["segment"].as_u64()?,
                stable: text("stable"),
                tentative: text("tentative"),
            },
            "segment" => StreamEvent::Segment(segment(v)?),
            "final" => StreamEvent::Final(final_of(v)?),
            "error" => StreamEvent::Error {
                code: text("code"),
                message: text("message"),
            },
            "cancelled" => StreamEvent::Cancelled,
            _ => return None,
        })
    }

    fn ends(&self) -> bool {
        matches!(
            self,
            StreamEvent::Final(_) | StreamEvent::Error { .. } | StreamEvent::Cancelled
        )
    }
}

fn segment(v: &Value) -> Option<Segment> {
    Some(Segment {
        segment: v["segment"].as_u64()?,
        start_ms: v["startMs"].as_u64()?,
        end_ms: v["endMs"].as_u64()?,
        text: v["text"].as_str()?.to_owned(),
    })
}

fn final_of(v: &Value) -> Option<Final> {
    let segments = v["segments"]
        .as_array()?
        .iter()
        .map(segment)
        .collect::<Option<_>>()?;
    Some(Final {
        text: v["text"].as_str()?.to_owned(),
        segments,
        duration_ms: v["durationMs"].as_u64()?,
    })
}

/// A stream's events in order, from its start, until it ends or the connection drops. Hex keeps
/// one reader per stream; a newer one ends this.
pub struct StreamEvents {
    lines: std::io::Lines<BufReader<Box<dyn Read + Send>>>,
    done: bool,
}

impl Iterator for StreamEvents {
    type Item = StreamEvent;

    fn next(&mut self) -> Option<StreamEvent> {
        while !self.done {
            let Some(Ok(line)) = self.lines.next() else {
                break;
            };
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let Some(event) = serde_json::from_str::<Value>(data.trim())
                .ok()
                .as_ref()
                .and_then(StreamEvent::parse)
            else {
                continue;
            };
            self.done = event.ends();
            return Some(event);
        }
        self.done = true;
        None
    }
}

impl Hex {
    /// One `/streams` request; a reply Hex refused comes back as its code.
    fn stream_call(
        &self,
        path: &str,
        s: Option<&Stream>,
        kind: &str,
        body: &[u8],
        wait: Duration,
    ) -> Result<Value, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(wait))
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = agent
            .post(format!("{}{path}", self.base))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Content-Type", kind);
        if let Some(s) = s {
            req = req.header("x-hex-stream-token", &s.owner);
        }
        let mut resp = req.send(body).map_err(|_| NOT_RUNNING.to_owned())?;
        let status = resp.status().as_u16();
        let v: Value = serde_json::from_str(&resp.body_mut().read_to_string().unwrap_or_default())
            .unwrap_or(Value::Null);
        if status >= 400 {
            return Err(v["code"].as_str().unwrap_or("refused").to_owned());
        }
        Ok(v)
    }

    /// Starts a stream of f32 samples at 16 kHz on whatever model Hex's dictation holds.
    pub fn stream(&self) -> Result<Stream, String> {
        let body = json!({ "sampleFormat": "f32le", "sampleRate": STREAM_RATE }).to_string();
        let v = self
            .stream_call(
                "/streams",
                None,
                "application/json",
                body.as_bytes(),
                Duration::from_secs(10),
            )
            .map_err(|code| match code.as_str() {
                "stream-busy" => STREAM_BUSY.to_owned(),
                "model-not-loaded" => NO_MODEL.to_owned(),
                "engine-unavailable" => NO_ENGINE.to_owned(),
                // A Hex from before streams.
                "not_found" => self.outdated.clone(),
                _ if code.contains("running") => code,
                _ => format!("Hex couldn't start a stream: {code}"),
            })?;
        let text = |k: &str| v[k].as_str().unwrap_or("").to_owned();
        match (v["id"].as_u64(), v["ownerToken"].as_str()) {
            (Some(id), Some(owner)) => Ok(Stream {
                id,
                owner: owner.to_owned(),
                model: text("model"),
                language: text("language"),
                partials: v["partials"] == true,
            }),
            _ => Err("Hex sent an odd reply".into()),
        }
    }

    /// Sends recorded samples (mono, 16 kHz, -1 to 1) and keeps the stream alive. Fails with
    /// Hex's code: [`BACKLOG_FULL`] means push the same samples again shortly.
    pub fn push(&self, s: &Stream, samples: &[f32]) -> Result<(), String> {
        for piece in samples.chunks(MAX_PUSH_SAMPLES) {
            let bytes: Vec<u8> = piece.iter().flat_map(|f| f.to_le_bytes()).collect();
            self.stream_call(
                &format!("/streams/{}/audio", s.id),
                Some(s),
                "application/octet-stream",
                &bytes,
                Duration::from_secs(10),
            )?;
        }
        Ok(())
    }

    /// Keeps a stream alive while nothing is pushed (Hex ends one after 10 s without either).
    pub fn stream_heartbeat(&self, s: &Stream) -> Result<(), String> {
        self.stream_call(
            &format!("/streams/{}/heartbeat", s.id),
            Some(s),
            "application/json",
            b"",
            Duration::from_secs(5),
        )
        .map(|_| ())
    }

    /// The stream's events, from its start until it ends. Read them on their own thread.
    pub fn stream_events(&self, s: &Stream) -> Result<StreamEvents, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .build()
            .into();
        let resp = agent
            .get(format!("{}/streams/{}/events", self.base, s.id))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("x-hex-stream-token", &s.owner)
            .call()
            .map_err(|_| NOT_RUNNING.to_owned())?;
        if resp.status().as_u16() >= 400 {
            return Err(format!("events refused: {}", resp.status().as_u16()));
        }
        let reader: Box<dyn Read + Send> = Box::new(resp.into_body().into_reader());
        Ok(StreamEvents {
            lines: BufReader::new(reader).lines(),
            done: false,
        })
    }

    /// Transcribes what's left and waits for the whole text. Hex's code on failure, such as
    /// `stream-finish-timeout` (still going: finish again) or the code the stream ended with.
    pub fn finish_stream(&self, s: &Stream) -> Result<Final, String> {
        let v = self.stream_call(
            &format!("/streams/{}/finish", s.id),
            Some(s),
            "application/json",
            b"",
            Duration::from_secs(130),
        )?;
        final_of(&v).ok_or_else(|| "Hex sent an odd reply".to_owned())
    }

    pub fn cancel_stream(&self, s: &Stream) {
        let _ = self.stream_call(
            &format!("/streams/{}/cancel", s.id),
            Some(s),
            "application/json",
            b"",
            Duration::from_secs(5),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;

    /// One request the fake Hex got.
    #[derive(Debug, Clone)]
    struct Got {
        line: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Got {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    /// A fake Hex that answers each request with `reply(request line)`: status, content type, body.
    fn fake(
        reply: impl Fn(&str) -> (u16, &'static str, String) + Send + 'static,
    ) -> (Hex, Arc<Mutex<Vec<Got>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let got = Arc::new(Mutex::new(Vec::new()));
        let seen = got.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { break };
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let h = h.trim_end();
                    if h.is_empty() {
                        break;
                    }
                    let (n, v) = h.split_once(':').unwrap();
                    headers.push((n.trim().to_owned(), v.trim().to_owned()));
                }
                let len = headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                    .map_or(0, |(_, v)| v.parse().unwrap());
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let line = line.trim_end().to_owned();
                let (status, kind, text) = reply(&line);
                seen.lock().unwrap().push(Got {
                    line,
                    headers,
                    body,
                });
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        let hex = Hex {
            base: format!("http://127.0.0.1:{port}"),
            token: "tok".into(),
            source: "Seer".into(),
            outdated: crate::OUTDATED.into(),
        };
        (hex, got)
    }

    fn created() -> String {
        json!({ "id": 7, "ownerToken": "hex_owner", "model": "parakeet_v3", "language": "auto", "sampleRate": 16000, "sampleFormat": "f32le", "partials": true, "leaseMs": 10000 }).to_string()
    }

    const FINAL: &str = r#"{"text":"hello there","segments":[{"segment":0,"startMs":0,"endMs":900,"text":"hello there"}],"durationMs":900}"#;

    #[test]
    fn a_stream_is_created_fed_finished_and_cancelled_with_its_owner_token() {
        let (hex, got) = fake(|line| match line {
            "POST /streams HTTP/1.1" => (201, "application/json", created()),
            l if l.contains("/finish") => (200, "application/json", FINAL.into()),
            _ => (204, "application/json", String::new()),
        });
        let s = hex.stream().unwrap();
        assert_eq!(
            (s.id, s.model.as_str(), s.partials),
            (7, "parakeet_v3", true)
        );
        hex.push(&s, &[0.5, -0.25]).unwrap();
        hex.stream_heartbeat(&s).unwrap();
        let fin = hex.finish_stream(&s).unwrap();
        assert_eq!(fin.text, "hello there");
        assert_eq!(fin.segments[0].end_ms, 900);
        hex.cancel_stream(&s);

        let got = got.lock().unwrap();
        let create: Value = serde_json::from_slice(&got[0].body).unwrap();
        assert_eq!(
            create,
            json!({ "sampleFormat": "f32le", "sampleRate": 16000 })
        );
        assert_eq!(got[0].header("authorization"), Some("Bearer tok"));
        assert_eq!(got[1].line, "POST /streams/7/audio HTTP/1.1");
        assert_eq!(
            got[1].header("content-type"),
            Some("application/octet-stream")
        );
        assert_eq!(
            got[1].body,
            [0.5f32.to_le_bytes(), (-0.25f32).to_le_bytes()].concat()
        );
        let lines: Vec<&str> = got.iter().map(|g| g.line.as_str()).collect();
        assert_eq!(
            lines[2..],
            [
                "POST /streams/7/heartbeat HTTP/1.1",
                "POST /streams/7/finish HTTP/1.1",
                "POST /streams/7/cancel HTTP/1.1"
            ]
        );
        assert!(
            got[1..]
                .iter()
                .all(|g| g.header("x-hex-stream-token") == Some("hex_owner"))
        );
    }

    #[test]
    fn long_pushes_go_in_pieces_hex_accepts() {
        let (hex, got) = fake(|line| {
            if line.starts_with("POST /streams ") {
                (201, "application/json", created())
            } else {
                (204, "application/json", String::new())
            }
        });
        let s = hex.stream().unwrap();
        hex.push(&s, &vec![0.0; MAX_PUSH_SAMPLES * 2 + 1]).unwrap();
        let sizes: Vec<usize> = got.lock().unwrap()[1..]
            .iter()
            .map(|g| g.body.len())
            .collect();
        assert_eq!(sizes, [256 * 1024, 256 * 1024, 4]);
    }

    #[test]
    fn events_arrive_in_order_and_end_with_the_final() {
        let sse = [
            r#"{"type":"ready","id":7,"partials":true}"#,
            r#"{"type":"partial","segment":0,"stable":"hello","tentative":" th"}"#,
            r#"{"type":"segment","segment":0,"startMs":0,"endMs":900,"text":"hello there"}"#,
            FINAL.replacen('{', r#"{"type":"final","#, 1).leak(),
            r#"{"type":"partial","segment":9,"stable":"after the end","tentative":""}"#,
        ]
        .iter()
        .map(|d| format!("data: {d}\n\n"))
        .collect::<String>();
        let sse = format!(": keep-alive\n\n{sse}");
        let (hex, got) = fake(move |line| {
            if line.starts_with("POST /streams ") {
                (201, "application/json", created())
            } else {
                (200, "text/event-stream", sse.clone())
            }
        });
        let s = hex.stream().unwrap();
        let events: Vec<StreamEvent> = hex.stream_events(&s).unwrap().collect();
        assert_eq!(
            events,
            [
                StreamEvent::Partial {
                    segment: 0,
                    stable: "hello".into(),
                    tentative: " th".into()
                },
                StreamEvent::Segment(Segment {
                    segment: 0,
                    start_ms: 0,
                    end_ms: 900,
                    text: "hello there".into()
                }),
                StreamEvent::Final(Final {
                    text: "hello there".into(),
                    segments: vec![Segment {
                        segment: 0,
                        start_ms: 0,
                        end_ms: 900,
                        text: "hello there".into()
                    }],
                    duration_ms: 900
                }),
            ]
        );
        let got = got.lock().unwrap();
        assert_eq!(got[1].line, "GET /streams/7/events HTTP/1.1");
        assert_eq!(got[1].header("x-hex-stream-token"), Some("hex_owner"));
    }

    #[test]
    fn hex_refusals_read_as_the_apps_words_or_hexs_code() {
        for (code, says) in [
            ("stream-busy", STREAM_BUSY),
            ("model-not-loaded", NO_MODEL),
            ("engine-unavailable", NO_ENGINE),
            ("not_found", crate::OUTDATED),
        ] {
            let body = json!({ "code": code }).to_string();
            let (hex, _) = fake(move |_| (409, "application/json", body.clone()));
            assert_eq!(hex.stream().unwrap_err(), says);
        }
        let (hex, _) = fake(|line| {
            if line.starts_with("POST /streams ") {
                (201, "application/json", created())
            } else {
                (
                    429,
                    "application/json",
                    r#"{"code":"stream-backlog-full"}"#.into(),
                )
            }
        });
        let s = hex.stream().unwrap();
        assert_eq!(hex.push(&s, &[0.0]).unwrap_err(), BACKLOG_FULL);
        let (hex, _) = fake(|line| {
            if line.starts_with("POST /streams ") {
                (201, "application/json", created())
            } else {
                (
                    409,
                    "application/json",
                    r#"{"code":"lease-expired","message":"no audio"}"#.into(),
                )
            }
        });
        let s = hex.stream().unwrap();
        assert_eq!(hex.finish_stream(&s).unwrap_err(), "lease-expired");
    }

    #[test]
    fn a_hex_that_isnt_running_says_so() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let hex = Hex {
            base: format!("http://127.0.0.1:{port}"),
            token: "tok".into(),
            source: "Seer".into(),
            outdated: crate::OUTDATED.into(),
        };
        assert_eq!(hex.stream().unwrap_err(), NOT_RUNNING);
    }
}
