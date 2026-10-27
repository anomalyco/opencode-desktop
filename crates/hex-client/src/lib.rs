//! Hex's desktop capture API (the one its SDK's `connect()` uses): an app borrows Hex's
//! microphone and model for one recording and gets the whole transcript back from `finish`.
//! Unlike `hex dictation start`, nothing ends it but the app (Smart Turn never stops it on a
//! pause), and nothing is pasted. Hex has no partial transcripts; while it listens it streams
//! the microphone's level. Hex writes where it listens to `local-api.json`, and removes the
//! file when it quits.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

pub struct Hex {
    base: String,
    token: String,
    /// The name Hex shows for who is recording.
    source: String,
}

#[derive(Clone)]
pub struct Capture {
    pub id: u64,
    owner: String,
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
        Ok(Hex { base: format!("http://127.0.0.1:{port}"), token: token.to_owned(), source: source.to_owned() })
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
            "service-capture-unavailable" | "not_found" => OUTDATED.to_owned(),
            NOT_RUNNING => code,
            _ => format!("Hex couldn't start: {code}"),
        })?;
        match (v["id"].as_u64(), v["ownerToken"].as_str()) {
            (Some(id), Some(owner)) => Ok(Capture { id, owner: owner.to_owned() }),
            _ => Err("Hex sent an odd reply".into()),
        }
    }

    /// Keeps the capture alive (Hex drops one that misses 10 s of heartbeats). Fails once Hex
    /// has ended it: Escape, or Hex quitting.
    pub fn heartbeat(&self, c: &Capture) -> Result<(), String> {
        self.call(&format!("/dictations/{}/heartbeat", c.id), Some(&c.owner), None, Duration::from_secs(5)).map(|_| ())
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
