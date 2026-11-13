//! Reliable control messages and one replaceable software paint per tab. A paint notification
//! occupies its place in the same FIFO as control messages; replacement never adds queue entries.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use base64::Engine as _;
use crate::wire::EngineMsg;

pub struct Paint {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

enum Entry {
    Message(EngineMsg),
    Paint(u64),
}

#[derive(Clone)]
pub struct Output {
    entries: mpsc::Sender<Entry>,
    pending: Arc<Mutex<HashMap<u64, Paint>>>,
}

impl Output {
    pub fn channel() -> (Self, impl Iterator<Item = EngineMsg>) {
        let (entries, rx) = mpsc::channel();
        let pending = Arc::new(Mutex::new(HashMap::<u64, Paint>::new()));
        let output = Self { entries, pending: pending.clone() };
        let messages = rx.into_iter().filter_map(move |entry| match entry {
            Entry::Message(msg) => Some(msg),
            Entry::Paint(tab_id) => {
                let paint = pending.lock().unwrap().remove(&tab_id)?;
                Some(EngineMsg::CpuFrame {
                    tab_id,
                    frame_id: paint.frame_id,
                    width: paint.width,
                    height: paint.height,
                    bgra_b64: base64::engine::general_purpose::STANDARD.encode(paint.bgra),
                })
            }
        });
        (output, messages)
    }

    pub fn send(&self, msg: EngineMsg) -> Result<(), ()> {
        if let EngineMsg::TabClosed { tab_id } = &msg {
            self.pending.lock().unwrap().remove(tab_id);
        }
        self.entries.send(Entry::Message(msg)).map_err(|_| ())
    }

    pub fn paint(&self, tab_id: u64, paint: Paint) {
        if self.pending.lock().unwrap().insert(tab_id, paint).is_none() {
            let _ = self.entries.send(Entry::Paint(tab_id));
        }
    }
}
