//! Hex's desktop capture API (the one its SDK's `connect()` uses): an app borrows Hex's
//! microphone and model for one recording and gets the whole transcript back from `finish`.
//! Unlike `hex dictation start`, nothing ends it but the app (Smart Turn never stops it on a
//! pause), and nothing is pasted. Hex has no partial transcripts; while it listens it streams
//! the microphone's level, and can stream the audio itself for a copy the app keeps. Hex writes
//! where it listens to `local-api.json`, and removes the file when it quits.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

/// How loud the microphone was in the last stretch `keep` wrote, 0–1, as f32 bits.
static LEVEL: AtomicU32 = AtomicU32::new(0);

/// `keep` is writing audio as it comes, so `level` is live.
static STREAMING: AtomicBool = AtomicBool::new(false);

/// How loud the speaker is now, for a recording's meter.
pub fn level() -> f32 {
    f32::from_bits(LEVEL.load(Ordering::Relaxed))
}

/// Audio is reaching `keep`, so a `level` of 0 means a silent mic, not a missing stream.
pub fn streaming() -> bool {
    STREAMING.load(Ordering::Relaxed)
}

pub struct Hex {
    base: String,
    token: String,
    /// The name Hex shows for who is recording.
    source: String,
    /// What `start` says when this Hex can't record (`OUTDATED` unless the app words it).
    outdated: String,
}

#[derive(Clone)]
pub struct Capture {
    pub id: u64,
    owner: String,
    /// The microphone's rate, for the copy of the audio the app keeps (`keep`).
    rate: u32,
}

pub const NOT_RUNNING: &str = "Hex isn't running";
pub const BUSY: &str = "Hex is already recording";
pub const OUTDATED: &str = "This Hex can't record and needs an update";

/// Hex's endpoint file: `SEER_HEX` names one (tests, or a Hex elsewhere), else Hex's own.
pub fn discovery() -> PathBuf {
    if let Some(p) = std::env::var_os("SEER_HEX") {
        return p.into();
    }
    let support = std::env::var_os("HEX_APPLICATION_SUPPORT_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/Application Support/hex")
    });
    support.join("local-api.json")
}

impl Hex {
    /// Hex at its usual endpoint file, recording for `source`.
    pub fn connect(source: &str) -> Result<Hex, String> {
        Self::connect_at(&discovery(), source)
    }

    pub fn connect_at(file: &Path, source: &str) -> Result<Hex, String> {
        let off = || NOT_RUNNING.to_owned();
        let v: Value = serde_json::from_slice(&std::fs::read(file).map_err(|_| off())?).map_err(|_| off())?;
        let (Some(port), Some(token)) = (v["port"].as_u64(), v["token"].as_str()) else { return Err(off()) };
        Ok(Hex { base: format!("http://127.0.0.1:{port}"), token: token.to_owned(), source: source.to_owned(), outdated: OUTDATED.to_owned() })
    }

    /// The app's own words for a Hex that can't record.
    pub fn outdated_says(mut self, words: &str) -> Hex {
        self.outdated = words.to_owned();
        self
    }

    /// One request; a reply Hex refused comes back as its code.
    fn call(&self, path: &str, owner: Option<&str>, body: Option<Value>, wait: Duration) -> Result<Value, String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(wait)).http_status_as_error(false).build().into();
        let mut req = agent.post(format!("{}{path}", self.base)).header("Authorization", &format!("Bearer {}", self.token)).header("Content-Type", "application/json");
        if let Some(o) = owner {
            req = req.header("x-hex-dictation-token", o);
        }
        let mut resp = req.send(body.unwrap_or(json!({})).to_string().as_bytes()).map_err(|_| NOT_RUNNING.to_owned())?;
        let status = resp.status().as_u16();
        let v: Value = serde_json::from_str(&resp.body_mut().read_to_string().unwrap_or_default()).unwrap_or(Value::Null);
        if status >= 400 {
            return Err(v["code"].as_str().unwrap_or("refused").to_owned());
        }
        Ok(v)
    }

    pub fn start(&self) -> Result<Capture, String> {
        let v = self.call("/dictations", None, Some(json!({ "source": self.source })), Duration::from_secs(10)).map_err(|code| match code.as_str() {
            "dictation-busy" => BUSY.to_owned(),
            // An older Hex has the API but no capture behind it, or no `/dictations` at all.
            "service-capture-unavailable" | "not_found" => self.outdated.clone(),
            _ if code.contains("running") => code,
            _ => format!("Hex couldn't start: {code}"),
        })?;
        match (v["id"].as_u64(), v["ownerToken"].as_str()) {
            (Some(id), Some(owner)) => Ok(Capture { id, owner: owner.to_owned(), rate: v["sampleRate"].as_u64().unwrap_or(48_000) as u32 }),
            _ => Err("Hex sent an odd reply".into()),
        }
    }

    /// Keeps the capture alive (Hex drops one that misses 10 s of heartbeats). Fails once Hex
    /// has ended it: Escape, or Hex quitting.
    pub fn heartbeat(&self, c: &Capture) -> Result<(), String> {
        self.call(&format!("/dictations/{}/heartbeat", c.id), Some(&c.owner), None, Duration::from_secs(5)).map(|_| ())
    }

    /// Writes what the microphone hears to `wav` until the capture ends (Hex streams it as f32
    /// samples), so a recording outlives a transcription or a Hex that goes away (t186).
    pub fn keep(&self, c: &Capture, wav: &Path) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_recv_response(Some(Duration::from_secs(5))).http_status_as_error(false).build().into();
        let resp = agent.get(format!("{}/dictations/{}/audio", self.base, c.id)).header("Authorization", &format!("Bearer {}", self.token)).header("x-hex-dictation-token", &c.owner).call().map_err(|e| e.to_string())?;
        if resp.status().as_u16() >= 400 {
            return Err(format!("Hex refused the audio ({})", resp.status()));
        }
        let mut body = resp.into_body().into_reader();
        let mut out = std::io::BufWriter::new(std::fs::File::create(wav).map_err(|e| e.to_string())?);
        out.write_all(&wav_header(c.rate, 0)).map_err(|e| e.to_string())?;
        let (mut buf, mut carry, mut samples) = (vec![0u8; 16 * 1024], Vec::new(), 0u32);
        loop {
            let n = match body.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            carry.extend_from_slice(&buf[..n]);
            let whole = carry.len() / 4 * 4;
            let mut power = 0.0f32;
            for b in carry[..whole].chunks_exact(4) {
                let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(-1.0, 1.0);
                power += f * f;
                out.write_all(&((f * i16::MAX as f32) as i16).to_le_bytes()).map_err(|e| e.to_string())?;
                samples += 1;
            }
            if whole > 0 {
                STREAMING.store(true, Ordering::Relaxed);
                // -50 dBFS (a quiet room) is 0 and -10 dBFS is 1, so a normal voice sits near half.
                let db = 10.0 * (power / (whole / 4) as f32).max(1e-10).log10();
                LEVEL.store(((db + 50.0) / 40.0).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            }
            carry.drain(..whole);
            // On disk as it comes, so an app that dies mid-sentence leaves the audio so far.
            out.flush().map_err(|e| e.to_string())?;
        }
        LEVEL.store(0, Ordering::Relaxed);
        STREAMING.store(false, Ordering::Relaxed);
        out.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        out.write_all(&wav_header(c.rate, samples)).map_err(|e| e.to_string())?;
        let file = out.into_inner().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())
    }

    /// Transcribes a kept recording with the first model Hex has ready.
    pub fn transcribe(&self, wav: &Path) -> Result<String, String> {
        let bytes = whole(std::fs::read(wav).map_err(|_| "the recording is gone".to_owned())?);
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
        let mut resp = agent.get(format!("{}/models", self.base)).header("Authorization", &format!("Bearer {}", self.token)).call().map_err(|_| NOT_RUNNING.to_owned())?;
        let models: Value = serde_json::from_str(&resp.body_mut().read_to_string().unwrap_or_default()).unwrap_or(Value::Null);
        let ready: Vec<&str> = models.as_array().into_iter().flatten().filter(|m| m["installed"] == true && m["verified"] == true).filter_map(|m| m["id"].as_str()).collect();
        let model = ready.iter().find(|id| id.contains("parakeet")).or(ready.first()).ok_or("Hex has no model ready")?;
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(130))).http_status_as_error(false).build().into();
        let mut resp = agent
            .post(format!("{}/transcriptions?model={model}", self.base))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Content-Type", "audio/wav")
            .send(&bytes[..])
            .map_err(|_| NOT_RUNNING.to_owned())?;
        let status = resp.status().as_u16();
        let v: Value = serde_json::from_str(&resp.body_mut().read_to_string().unwrap_or_default()).unwrap_or(Value::Null);
        if status >= 400 {
            return Err(format!("Hex couldn't transcribe: {}", v["code"].as_str().unwrap_or("refused")));
        }
        Ok(v["transcript"].as_str().unwrap_or("").to_owned())
    }

    /// Stops the microphone and waits for the full transcript.
    pub fn finish(&self, c: &Capture) -> Result<String, String> {
        let v = self.call(&format!("/dictations/{}/finish", c.id), Some(&c.owner), None, Duration::from_secs(130)).map_err(|code| match code.as_str() {
            "dictation-not-active" | "dictation-not-found" => "Hex stopped the recording".to_owned(),
            _ => format!("Hex couldn't finish: {code}"),
        })?;
        Ok(v["transcript"].as_str().unwrap_or("").to_owned())
    }

    pub fn cancel(&self, c: &Capture) {
        let _ = self.call(&format!("/dictations/{}/cancel", c.id), Some(&c.owner), None, Duration::from_secs(5));
    }

    /// The microphone's level while the capture runs, as Hex measures it: `on` gets each
    /// reading's RMS in dBFS and returns false to stop listening. Returns when the capture ends.
    /// Hex hands the stream to one listener per capture.
    pub fn levels(&self, c: &Capture, mut on: impl FnMut(f32) -> bool) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(5))).http_status_as_error(false).build().into();
        let resp = agent
            .get(format!("{}/dictations/{}/levels", self.base, c.id))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("x-hex-dictation-token", &c.owner)
            .call()
            .map_err(|_| NOT_RUNNING.to_owned())?;
        if resp.status().as_u16() >= 400 {
            return Err(format!("levels refused: {}", resp.status().as_u16()));
        }
        for line in BufReader::new(resp.into_body().into_reader()).lines() {
            let Ok(line) = line else { break };
            let Some(data) = line.strip_prefix("data:") else { continue };
            let Some(db) = serde_json::from_str::<Value>(data.trim()).ok().and_then(|v| v["rmsDb"].as_f64()) else { continue };
            if !on(db as f32) {
                break;
            }
        }
        Ok(())
    }
}

/// A kept WAV with its sizes set from its length: one the app stopped writing mid-stream still
/// says it holds no samples.
pub fn whole(mut wav: Vec<u8>) -> Vec<u8> {
    if wav.len() >= 44 && &wav[..4] == b"RIFF" && &wav[36..40] == b"data" {
        let data = (wav.len() - 44) as u32 / 2 * 2;
        wav[4..8].copy_from_slice(&(36 + data).to_le_bytes());
        wav[40..44].copy_from_slice(&data.to_le_bytes());
        wav.truncate(44 + data as usize);
    }
    wav
}

/// A 16-bit mono WAV header for `samples` samples at `rate`.
pub fn wav_header(rate: u32, samples: u32) -> [u8; 44] {
    let data = samples * 2;
    let mut h = [0u8; 44];
    let mut put = |at: usize, b: &[u8]| h[at..at + b.len()].copy_from_slice(b);
    put(0, b"RIFF");
    put(4, &(36 + data).to_le_bytes());
    put(8, b"WAVEfmt ");
    put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes());
    put(22, &1u16.to_le_bytes());
    put(24, &rate.to_le_bytes());
    put(28, &(rate * 2).to_le_bytes());
    put(32, &2u16.to_le_bytes());
    put(34, &16u16.to_le_bytes());
    put(36, b"data");
    put(40, &data.to_le_bytes());
    h
}
