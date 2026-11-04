//! A stand-in OpenCode server for the new-session benchmark: it answers the calls a start makes
//! after a set delay, as a local or a distant server would. Each prompt starts a run whose first
//! step begins `STEP` ms after the prompt is answered, on the event stream and in the session's
//! first page.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Sender, channel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long each kind of call takes the server, before the round trip.
#[derive(Clone, Copy)]
pub struct Latency {
    pub name: &'static str,
    pub create: u64,
    pub switch: u64,
    pub prompt: u64,
    pub read: u64,
    pub rtt: u64,
}

pub const LOCAL: Latency = Latency { name: "local", create: 25, switch: 5, prompt: 15, read: 5, rtt: 0 };
pub const REMOTE: Latency = Latency { name: "remote", rtt: 60, ..LOCAL };

/// How long after its prompt is answered a run's first step starts.
const STEP: u64 = 20;

/// Each session's first prompt (its id and text) and when its first step starts.
type Prompts = Arc<Mutex<HashMap<String, (String, String, Instant)>>>;
/// The open event streams.
type Streams = Arc<Mutex<Vec<Sender<String>>>>;

/// A running mock; dropping it closes its event streams and stops it taking calls.
pub struct Mock {
    pub base: String,
    closed: Arc<AtomicBool>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        // Wakes the accept loop so it sees it.
        let _ = TcpStream::connect(self.base.trim_start_matches("http://"));
    }
}

/// Serves on a free port.
pub fn serve(latency: Latency) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let prompts: Prompts = Arc::default();
    let streams: Streams = Arc::default();
    let closed = Arc::new(AtomicBool::new(false));
    let shut = closed.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if shut.load(Ordering::Relaxed) {
                return;
            }
            let (prompts, streams, shut) = (prompts.clone(), streams.clone(), shut.clone());
            std::thread::spawn(move || answer(stream, latency, &prompts, &streams, &shut));
        }
    });
    Mock { base, closed }
}

fn answer(mut s: TcpStream, l: Latency, prompts: &Prompts, streams: &Streams, closed: &AtomicBool) {
    let Some((method, path, body)) = request(&mut s) else { return };
    let path = path.split('?').next().unwrap_or("").to_owned();
    let parts: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    if path == "/api/event" {
        let (tx, rx) = channel();
        streams.lock().unwrap().push(tx);
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n");
        while !closed.load(Ordering::Relaxed) {
            let line = rx.recv_timeout(Duration::from_millis(100)).map_or_else(|_| ": ping\n\n".to_owned(), |e| format!("data: {e}\n\n"));
            if s.write_all(line.as_bytes()).is_err() { return; }
        }
        return;
    }
    let (wait, reply) = match (method.as_str(), parts.as_slice()) {
        ("POST", ["session"]) => {
            let id = body["id"].as_str().map_or_else(|| format!("ses_mock{:010}", rand()), str::to_owned);
            let mut info = json!({ "id": id, "title": "New session", "location": { "directory": body["location"]["directory"] }, "time": { "created": 1, "updated": 1 } });
            if let Some(a) = body["agent"].as_str() { info["agent"] = a.into(); }
            if body["model"].is_object() { info["model"] = body["model"].clone(); }
            (l.create, json!({ "data": info }))
        }
        ("POST", ["session", _, "agent" | "model"]) => (l.switch, json!({ "data": {} })),
        ("POST", ["session", id, "prompt"]) => {
            let pid = body["id"].as_str().unwrap_or("msg_mock").to_owned();
            let step = Instant::now() + Duration::from_millis(l.prompt + l.rtt / 2 + STEP);
            prompts.lock().unwrap().insert((*id).to_owned(), (pid.clone(), body["text"].as_str().unwrap_or("").to_owned(), step));
            let (id, streams) = ((*id).to_owned(), streams.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(l.prompt + l.rtt / 2 + STEP));
                for e in [json!({ "type": "session.execution.started", "data": { "sessionID": id } }), json!({ "type": "session.step.started", "data": { "sessionID": id, "assistantMessageID": reply_id(&id), "agent": "build" } })] {
                    streams.lock().unwrap().retain(|s| s.send(e.to_string()).is_ok());
                }
            });
            (l.prompt, json!({ "data": { "id": pid } }))
        }
        ("GET", ["session", id, "message"]) => {
            let mut messages = Vec::new();
            if let Some((pid, text, step)) = prompts.lock().unwrap().get(*id) {
                if Instant::now() >= *step {
                    messages.push(json!({ "id": reply_id(id), "type": "assistant", "time": { "created": 2 } }));
                }
                messages.push(json!({ "id": pid, "type": "user", "text": text }));
            }
            (l.read, json!({ "data": messages }))
        }
        ("GET", _) => (l.read, json!({ "data": [] })),
        _ => (l.switch, json!({ "data": {} })),
    };
    std::thread::sleep(Duration::from_millis(wait + l.rtt));
    let body = reply.to_string();
    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
}

fn reply_id(session: &str) -> String {
    format!("msg_zzzz{}", &session[4..])
}

fn rand() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish() % 10_000_000_000
}

/// The method, path and JSON body of the request on `s`.
fn request(s: &mut TcpStream) -> Option<(String, String, Value)> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk).ok().filter(|n| *n > 0)?;
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        let Some(h) = text.find("\r\n\r\n") else { continue };
        let len = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0usize);
        if buf.len() < h + 4 + len { continue; }
        let mut first = text.lines().next()?.split(' ');
        let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
        let body = serde_json::from_slice(&buf[h + 4..h + 4 + len]).unwrap_or(Value::Null);
        return Some((method, path, body));
    }
}
