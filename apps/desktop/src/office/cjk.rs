//! The Chinese, Japanese and Korean faces, fetched the first time a file needs them rather than
//! shipped (27 MB): until they are installed those scripts draw with the Latin faces, as in
//! Desktop, which has none. Once installed, every open file lays out again with them.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use office_preview::fonts::FontPacks;

/// Where the pack stands for this run.
enum Pack {
    /// Not looked for yet.
    Unknown,
    /// Not installed, and no file has needed it.
    Missing,
    /// Fetching, answered on the receiver.
    Installing(Receiver<Result<PathBuf, String>>),
    Ready(PathBuf),
    /// The fetch failed this run; a restart tries again.
    Failed,
}

thread_local! {
    static PACK: RefCell<Pack> = const { RefCell::new(Pack::Unknown) };
}

fn fonts_dir() -> Option<PathBuf> {
    crate::dirs::cache().map(|c| c.join("office-fonts"))
}

/// The faces files open with now.
pub fn packs() -> FontPacks {
    PACK.with_borrow_mut(|pack| {
        if matches!(pack, Pack::Unknown) {
            *pack = match fonts_dir().filter(|d| office_preview::cjk::installed(d)) {
                Some(dir) => Pack::Ready(office_preview::cjk::dir(&dir)),
                None => Pack::Missing,
            };
        }
        FontPacks::new(match pack {
            Pack::Ready(dir) => Some(dir.as_path()),
            _ => None,
        })
    })
}

/// A file needs the CJK faces: fetches them unless they are here or on their way.
pub fn want() {
    let _ = packs();
    PACK.with_borrow_mut(|pack| {
        if !matches!(pack, Pack::Missing) || crate::headless() {
            return;
        }
        let Some(dir) = fonts_dir() else { return };
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::thread::Builder::new().name("office-fonts".into()).spawn(move || {
            tx.send(office_preview::cjk::install(&dir, &fetch)).ok();
            super::registry::redraw();
        });
        *pack = if started.is_ok() { Pack::Installing(rx) } else { Pack::Failed };
    });
}

/// Takes the fetch's answer; true when the faces arrived and every file opens again.
pub fn poll() -> bool {
    let arrived = PACK.with_borrow_mut(|pack| {
        let Pack::Installing(rx) = pack else { return false };
        match rx.try_recv() {
            Ok(Ok(dir)) => {
                *pack = Pack::Ready(dir);
                true
            }
            Ok(Err(e)) => {
                eprintln!("office: the CJK faces could not be installed: {e}");
                *pack = Pack::Failed;
                false
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                *pack = Pack::Failed;
                false
            }
        }
    });
    if arrived {
        super::registry::reopen_all();
    }
    arrived
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(300))).build().into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    resp.body_mut().with_config().limit(16 << 20).read_to_vec().map_err(|e| e.to_string())
}
