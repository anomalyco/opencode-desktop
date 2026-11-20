//! Times a page load the way a browser tab sees it: engine up, first frame on the surface,
//! loading done. On Linux, use a private directory for the socket and a separate profile:
//! ```sh
//! d=$(mktemp -d /tmp/browser-pane.XXXXXX) && \
//! OPENCODE_BROWSER_SOCKET="$d/x.sock" OPENCODE_BROWSER_PROFILE="$d/profile" \
//! cargo run -p browser-pane --features engine --example load_timing -- URL…
//! ```

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use browser_pane::{BrowserHost, EngineStatus};

fn main() {
    let urls: Vec<String> = std::env::args().skip(1).collect();
    let mut host = BrowserHost::new();
    host.set_waker(Arc::new(|| {}));
    let start = Instant::now();
    let first_frame: Arc<Mutex<Option<Instant>>> = Arc::default();
    let mut id = None;
    for url in &urls {
        let t0 = Instant::now();
        *first_frame.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let tab = match id {
            None => host.open_tab(url),
            Some(id) => {
                host.navigate(id, url);
                id
            }
        };
        id = Some(tab);
        let mark = first_frame.clone();
        let Some(t) = host.tab(tab) else {
            eprintln!("{url}: tab {tab} was closed");
            break;
        };
        t.surface.set_waker(Arc::new(move || {
            mark.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(Instant::now);
        }));
        let mut ready = None;
        let mut seen_loading = false;
        let done = loop {
            host.poll();
            if ready.is_none() && host.status() == EngineStatus::Ready && start.elapsed() > Duration::ZERO {
                ready = Some(t0.elapsed());
            }
            let Some(t) = host.tab(tab) else {
                eprintln!("{url}: tab {tab} closed while loading");
                break t0.elapsed();
            };
            seen_loading |= t.loading;
            if seen_loading && !t.loading {
                break t0.elapsed();
            }
            if t0.elapsed() > Duration::from_secs(30) {
                break t0.elapsed();
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        std::thread::sleep(Duration::from_millis(300));
        let frame = first_frame.lock().unwrap_or_else(PoisonError::into_inner).map(|f| f - t0);
        println!("{url}: first frame {frame:?}, loading done {done:?}");
    }
    host.shutdown_sidecar();
    std::thread::sleep(Duration::from_millis(500));
}
