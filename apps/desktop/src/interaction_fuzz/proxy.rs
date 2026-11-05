//! A TCP proxy between the app and a real server, for replaying against one ([`super::real`]):
//! the same network the link plays for the sim. Dead holds every byte until the network comes
//! back (a stall, not a reset), Slow delays each chunk two seconds, an outage refuses new
//! connections and cuts the open ones, and a POST can lose its reply or never arrive: the bytes
//! vanish and the connection stays open, so the client times out, as the sim's link does.

use std::io::{Read, Write as _};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Fine,
    Slow,
    Dead,
    /// Refuses new connections and has cut the open ones.
    Down,
}

const SLOW: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Shared {
    mode: Mutex<Mode>,
    conns: Mutex<Vec<TcpStream>>,
    drop_next_response: AtomicBool,
    lose_next_write: AtomicBool,
    log: Mutex<Vec<String>>,
}

pub struct Proxy {
    pub port: u16,
    shared: Arc<Shared>,
}

impl Proxy {
    pub fn start(upstream: SocketAddr) -> Proxy {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the proxy");
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::new(Shared::default());
        let start = Instant::now();
        let s = shared.clone();
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                if *s.mode.lock().unwrap() == Mode::Down {
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                }
                let Ok(server) = TcpStream::connect(upstream) else {
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                };
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                {
                    let mut conns = s.conns.lock().unwrap();
                    conns.retain(|c| c.peer_addr().is_ok());
                    conns.extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                }
                let lose_reply = Arc::new(AtomicBool::new(false));
                pipe(s.clone(), client.try_clone().unwrap(), server.try_clone().unwrap(), Dir::Request(lose_reply.clone()), start);
                pipe(s.clone(), server, client, Dir::Reply(lose_reply), start);
            }
        });
        Proxy { port, shared }
    }

    pub fn mode(&self) -> Mode {
        *self.shared.mode.lock().unwrap()
    }

    pub fn set(&self, mode: Mode) {
        *self.shared.mode.lock().unwrap() = mode;
        if mode == Mode::Down {
            for c in self.shared.conns.lock().unwrap().drain(..) {
                let _ = c.shutdown(Shutdown::Both);
            }
        }
    }

    pub fn drop_next_response(&self) {
        self.shared.drop_next_response.store(true, Ordering::SeqCst);
    }

    pub fn lose_next_write(&self) {
        self.shared.lose_next_write.store(true, Ordering::SeqCst);
    }

    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.log.lock().unwrap())
    }
}

/// Which way a pipe carries bytes, with the connection's flag for a reply to lose.
enum Dir {
    Request(Arc<AtomicBool>),
    Reply(Arc<AtomicBool>),
}

/// One direction of a connection: a reader that stamps chunks and a writer that sends each
/// once the network lets it (two seconds late when slow, held while dead).
fn pipe(s: Arc<Shared>, mut from: TcpStream, to: TcpStream, dir: Dir, start: Instant) {
    let (tx, rx) = channel::<(Instant, Vec<u8>)>();
    let reader_shared = s.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 << 10];
        let mut swallow = false;
        loop {
            let n = match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if swallow {
                continue;
            }
            let chunk = buf[..n].to_vec();
            let at = start.elapsed().as_secs_f64();
            match &dir {
                Dir::Request(lose_reply) if chunk.starts_with(b"POST ") => {
                    let end = chunk.iter().position(|b| *b == b'\r').unwrap_or(n.min(80));
                    let line = String::from_utf8_lossy(&chunk[..end]).into_owned();
                    if reader_shared.lose_next_write.swap(false, Ordering::SeqCst) {
                        reader_shared.log.lock().unwrap().push(format!("{at:.2}s proxy lost on the way: {line}"));
                        swallow = true;
                        continue;
                    }
                    if reader_shared.drop_next_response.swap(false, Ordering::SeqCst) {
                        reader_shared.log.lock().unwrap().push(format!("{at:.2}s proxy will lose the reply to: {line}"));
                        lose_reply.store(true, Ordering::SeqCst);
                    }
                }
                Dir::Reply(lose_reply) if lose_reply.swap(false, Ordering::SeqCst) => {
                    reader_shared.log.lock().unwrap().push(format!("{at:.2}s proxy lost a reply"));
                    swallow = true;
                    continue;
                }
                _ => {}
            }
            if tx.send((Instant::now(), chunk)).is_err() {
                break;
            }
        }
    });
    writer(s, rx, to);
}

fn writer(s: Arc<Shared>, rx: Receiver<(Instant, Vec<u8>)>, mut to: TcpStream) {
    std::thread::spawn(move || {
        for (at, chunk) in rx {
            loop {
                let mode = *s.mode.lock().unwrap();
                let due = if mode == Mode::Slow { at + SLOW } else { at };
                if mode == Mode::Dead || Instant::now() < due {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                break;
            }
            if to.write_all(&chunk).is_err() {
                break;
            }
        }
        // A dead network carries no close either: the other end hears of it once it's back.
        while *s.mode.lock().unwrap() == Mode::Dead {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = to.shutdown(Shutdown::Both);
    });
}
