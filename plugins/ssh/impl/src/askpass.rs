//! The sign-in listener (`askpass.ts`): a loopback port `ssh`'s askpass helper (the app's own
//! executable) sends each prompt to, with a token. Each prompt waits for the user's answer in the
//! app, one at a time, up to 5 minutes; a helper that goes away clears its prompt. It runs on a
//! host worker for as long as the connect does, polling for helpers every 50 ms, and stops when
//! the attempt ends.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use opencode_plugin::PluginWaker;

use crate::machine::Prompt;

/// What the listener tells the app's thread.
pub enum Note {
    Prompt(Prompt),
    Cleared(String),
}

/// Answers the user gave, by prompt id, and the condition the listener waits on.
type Answers = (Mutex<Vec<(String, String)>>, Condvar);

/// One attempt's listener, as the app's thread holds it.
pub struct Askpass {
    pub port: u16,
    pub token: String,
    stop: Arc<AtomicBool>,
    answers: Arc<Answers>,
    pub notes: Arc<Mutex<Vec<Note>>>,
}

impl Askpass {
    /// Listens on a free loopback port; `waker` wakes the plugin when a prompt comes or goes.
    pub fn start(waker: PluginWaker) -> Option<Askpass> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        listener.set_nonblocking(true).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = crate::token();
        let stop = Arc::new(AtomicBool::new(false));
        let answers = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let notes = Arc::new(Mutex::new(Vec::new()));
        let (s, a, n, t) = (stop.clone(), answers.clone(), notes.clone(), token.clone());
        opencode_plugin::spawn_after(0, move || serve(&listener, &t, &s, &a, &n, &waker));
        Some(Askpass { port, token, stop, answers, notes })
    }

    /// The user's answer to prompt `id`.
    pub fn respond(&self, id: &str, value: &str) {
        let (lock, cv) = &*self.answers;
        if let Ok(mut a) = lock.lock() {
            a.push((id.to_owned(), value.to_owned()));
        }
        cv.notify_all();
    }

    pub fn take_notes(&self) -> Vec<Note> {
        self.notes.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
    }

    /// `SSH_ASKPASS` and its friends, for each `ssh` the attempt runs.
    pub fn env(&self) -> Vec<(String, String)> {
        let exe = opencode_plugin::system::exe().unwrap_or_default();
        vec![
            ("SSH_ASKPASS".into(), exe),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            ("DISPLAY".into(), "opencode".into()),
            ("OPENCODE_SSH_ASKPASS_PORT".into(), self.port.to_string()),
            ("OPENCODE_SSH_ASKPASS_TOKEN".into(), self.token.clone()),
        ]
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.answers.1.notify_all();
    }
}

fn serve(listener: &TcpListener, token: &str, stop: &AtomicBool, answers: &Answers, notes: &Mutex<Vec<Note>>, waker: &PluginWaker) {
    let mut next = 0u64;
    let note = |n: Note| {
        if let Ok(mut all) = notes.lock() {
            all.push(n);
        }
        waker.wake();
    };
    while !stop.load(Ordering::Acquire) {
        let Ok((stream, _)) = listener.accept() else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut line = String::new();
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        });
        if reader.read_line(&mut line).is_err() || line.len() > 16_384 {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
        if v["token"].as_str() != Some(token) {
            continue;
        }
        next += 1;
        let id = format!("p{next}");
        note(Note::Prompt(Prompt { id: id.clone(), text: v["text"].as_str().unwrap_or_default().to_owned(), confirm: v["confirm"].as_bool().unwrap_or(false) }));
        let started = Instant::now();
        let (lock, cv) = answers;
        let mut value = None;
        if let Ok(mut a) = lock.lock() {
            while value.is_none() && !stop.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(300) {
                if let Some(at) = a.iter().position(|(p, _)| *p == id) {
                    value = Some(a.remove(at).1);
                    break;
                }
                a = match cv.wait_timeout(a, Duration::from_millis(500)) {
                    Ok((a, _)) => a,
                    Err(_) => break,
                };
            }
        }
        if let Some(value) = value {
            let mut stream = stream;
            stream.write_all(format!("{}\n", serde_json::json!({ "value": value })).as_bytes()).ok();
        }
        note(Note::Cleared(id));
    }
}
