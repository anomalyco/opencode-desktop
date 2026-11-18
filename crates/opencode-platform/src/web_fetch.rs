//! A plain web GET for pictures a reply shows from the web (a markdown image's URL, a link's
//! favicon), as Desktop's renderer loads an `<img>`: no credentials, a short timeout, at most
//! 8 MB, and only a successful reply counts.
//!
//! As the browser's cache does for Desktop, a picture read once is kept for the process (up to
//! 256 of them and 32 MB), so the timeline's sources chip and the Sources panel, each with its own
//! image store, read a favicon off the network once; a second reader of a URL still being read
//! waits for that read rather than starting its own. One agent serves every read, so its TLS
//! setup and connections are shared.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use ureq_web as ureq;

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 32 << 20;

#[derive(Default)]
struct Cache {
    done: HashMap<String, Arc<[u8]>>,
    /// Oldest first, for eviction.
    order: VecDeque<String>,
    bytes: usize,
    /// URLs being read now.
    reading: Vec<String>,
}

static CACHE: OnceLock<(Mutex<Cache>, Condvar)> = OnceLock::new();

pub fn get(url: &str) -> Result<Vec<u8>, String> {
    let (lock, landed) = CACHE.get_or_init(Default::default);
    let mut cache = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(bytes) = cache.done.get(url) {
            return Ok(bytes.to_vec());
        }
        if !cache.reading.iter().any(|u| u == url) {
            break;
        }
        cache = landed.wait(cache).unwrap_or_else(|e| e.into_inner());
    }
    cache.reading.push(url.to_owned());
    drop(cache);
    let read = fetch(url);
    let mut cache = lock.lock().unwrap_or_else(|e| e.into_inner());
    cache.reading.retain(|u| u != url);
    if let Ok(bytes) = &read {
        keep(&mut cache, url, bytes);
    }
    landed.notify_all();
    read
}

fn keep(cache: &mut Cache, url: &str, bytes: &[u8]) {
    if bytes.len() > MAX_BYTES {
        return;
    }
    while cache.done.len() >= MAX_ENTRIES || cache.bytes + bytes.len() > MAX_BYTES {
        let Some(old) = cache.order.pop_front() else { break };
        cache.bytes -= cache.done.remove(&old).map_or(0, |b| b.len());
    }
    cache.bytes += bytes.len();
    cache.done.insert(url.to_owned(), bytes.into());
    cache.order.push_back(url.to_owned());
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    let agent = AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(true)
            .build()
            .into()
    });
    let mut resp = agent.get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(8 << 20)
        .read_to_vec()
        .map_err(|e| format!("GET {url}: {e}"))
}
