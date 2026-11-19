//! The engine archive as one stream of bytes, read once: first whatever an earlier try left in
//! the partial file, then the rest from the source (an HTTP `Range` from where it ended), each
//! new byte written to the partial file as it passes, so a cut-short download resumes. The
//! reader hashes everything it hands on, so the SHA-256 covers the whole archive however many
//! tries it took, and the archive unpacks while it downloads.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;
use rune::time::Instant;

use sha2::{Digest, Sha256};

use crate::installer::{Fault, Net, err_write};
use crate::source;

/// How fast the next bytes may come, asked before each read from the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pace {
    Full,
    /// At most this many bytes a second.
    Limit(u64),
    /// Stop now and keep the partial file for later.
    Stop,
}

/// Why the stream ended before the archive did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cut {
    /// `pace` said stop; the partial file keeps what came.
    Stopped,
    /// The network or the disk failed. The bytes so far stay.
    Failed(Fault),
    /// The bytes don't check out (size, hash, archive, trust rule). The partial file goes, so
    /// the next try starts over.
    Rejected(Fault),
}

/// HTTP statuses that mean someone refused the download (a proxy, a filter, the server), not
/// that it broke.
pub(crate) fn blocked(code: u16) -> bool {
    matches!(code, 401 | 403 | 407 | 451)
}

/// The agent every engine request goes through: the system's certificate store (so a network
/// that inspects TLS with its own root, as companies' do, still works) and the proxy the
/// environment names.
pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .build()
        .new_agent()
}

pub(crate) struct Body<'a> {
    partial: File,
    /// Bytes of the partial file not yet handed on.
    replay: u64,
    source: Option<Box<dyn Read + Send>>,
    hasher: Sha256,
    /// Bytes handed on so far (the replayed ones too).
    have: u64,
    /// Bytes the partial file held when it opened: progress never shows less.
    resumed_at: u64,
    total: u64,
    progress: &'a mut dyn FnMut(u64, u64),
    pace: &'a mut dyn FnMut() -> Pace,
    window: (Instant, u64, Pace),
    last_notify: u64,
    cut: Option<Cut>,
}

impl<'a> Body<'a> {
    /// Opens `url` (`total` bytes, as the index says) after what `partial` already holds.
    /// `progress` hears `(downloaded, total)` every 256 KB of new bytes.
    pub(crate) fn open(
        url: &str,
        partial: &Path,
        total: u64,
        progress: &'a mut dyn FnMut(u64, u64),
        pace: &'a mut dyn FnMut() -> Pace,
    ) -> Result<Self, Fault> {
        if let Some(dir) = partial.parent() {
            std::fs::create_dir_all(dir).map_err(|e| err_write(&e))?;
        }
        let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(partial).map_err(|e| err_write(&e))?;
        // Locked while open, so two apps sharing the engine folder never write it at once.
        file.try_lock().map_err(|_| Fault::Busy)?;
        let mut have = file.metadata().map_or(0, |m| m.len());
        if have > total {
            file.set_len(0).map_err(|e| err_write(&e))?;
            have = 0;
        }
        let source = if have == total {
            None
        } else {
            match open_source(url, have)? {
                Opened::From(reader) => Some(reader),
                Opened::Whole(reader) => {
                    file.set_len(0).map_err(|e| err_write(&e))?;
                    have = 0;
                    Some(reader)
                }
            }
        };
        file.seek(SeekFrom::Start(0)).map_err(|e| err_write(&e))?;
        progress(have, total);
        Ok(Self {
            partial: file,
            replay: have,
            source,
            hasher: Sha256::new(),
            have: 0,
            resumed_at: have,
            total,
            progress,
            pace,
            window: (Instant::now(), 0, Pace::Full),
            last_notify: have,
            cut: None,
        })
    }

    /// Bytes an earlier try left in the partial file.
    pub(crate) fn resumed_at(&self) -> u64 {
        self.resumed_at
    }

    /// Why the stream stopped early, once a read failed.
    pub(crate) fn cut(&self) -> Option<&Cut> {
        self.cut.as_ref()
    }

    /// Reads whatever the consumer left (an archive's zero padding), checks the size, and
    /// returns the SHA-256 (lowercase hex).
    pub(crate) fn finish(mut self) -> Result<String, Cut> {
        let mut rest = [0u8; 64 * 1024];
        loop {
            match self.read(&mut rest) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => return Err(self.cut.take().unwrap_or_else(|| Cut::Failed(self.interrupted()))),
            }
        }
        self.partial.flush().ok();
        if self.have != self.total {
            return Err(Cut::Rejected(Fault::Corrupt));
        }
        (self.progress)(self.have, self.total);
        Ok(format!("{:x}", self.hasher.finalize()))
    }

    /// The connection ended part-way: what the partial file keeps for the next try.
    fn interrupted(&self) -> Fault {
        Fault::Interrupted { have: self.have.max(self.resumed_at), total: self.total }
    }

    fn fail(&mut self, cut: Cut) -> std::io::Error {
        self.partial.flush().ok();
        self.cut = Some(cut);
        std::io::Error::other("engine download cut short")
    }

    fn read_source(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let limit = match (self.pace)() {
            Pace::Stop => return Err(self.fail(Cut::Stopped)),
            p => p,
        };
        if limit != self.window.2 {
            self.window = (Instant::now(), 0, limit);
        }
        if let Pace::Limit(rate) = limit {
            // Hold the window's average at `rate`: sleep until its bytes are due.
            let due = Duration::from_secs_f64(self.window.1 as f64 / rate.max(1) as f64);
            if let Some(wait) = due.checked_sub(self.window.0.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let chunk = match limit {
            Pace::Limit(rate) => buf.len().min((rate / 8).max(4096) as usize),
            _ => buf.len(),
        };
        let Some(source) = self.source.as_mut() else { return Ok(0) };
        let n = match source.read(&mut buf[..chunk]) {
            Ok(n) => n,
            Err(_) => {
                let cut = Cut::Failed(self.interrupted());
                return Err(self.fail(cut));
            }
        };
        if n == 0 {
            return Ok(0);
        }
        if let Err(e) = self.partial.write_all(&buf[..n]) {
            let msg = err_write(&e);
            return Err(self.fail(Cut::Failed(msg)));
        }
        self.window.1 += n as u64;
        Ok(n)
    }
}

impl Read for Body<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cut.is_some() || buf.is_empty() {
            return Err(std::io::Error::other("engine download cut short"));
        }
        let n = if self.replay > 0 {
            let want = buf.len().min(self.replay.min(usize::MAX as u64) as usize);
            match self.partial.read(&mut buf[..want]) {
                Ok(0) => return Err(self.fail(Cut::Rejected(Fault::Corrupt))),
                Err(e) => return Err(self.fail(Cut::Failed(err_write(&e)))),
                Ok(n) => {
                    self.replay -= n as u64;
                    n
                }
            }
        } else {
            self.read_source(buf)?
        };
        if self.have + n as u64 > self.total {
            return Err(self.fail(Cut::Rejected(Fault::Corrupt)));
        }
        self.hasher.update(&buf[..n]);
        self.have += n as u64;
        if self.have >= self.last_notify + 256 * 1024 {
            self.last_notify = self.have;
            (self.progress)(self.have.max(self.resumed_at), self.total);
        }
        Ok(n)
    }
}

enum Opened {
    /// The source answers from the requested offset.
    From(Box<dyn Read + Send>),
    /// The source sends the whole archive again.
    Whole(Box<dyn Read + Send>),
}

fn open_source(url: &str, from: u64) -> Result<Opened, Fault> {
    if let Some(path) = source::file_path(url) {
        let mut file = File::open(path).map_err(|_| Fault::NotPublished)?;
        file.seek(SeekFrom::Start(from)).map_err(|e| err_write(&e))?;
        return Ok(Opened::From(Box::new(file)));
    }
    let agent = agent(Duration::from_secs(30));
    let mut req = agent.get(url);
    if from > 0 {
        req = req.header("Range", format!("bytes={from}-"));
    }
    match req.call() {
        Ok(resp) => {
            let resumed = from > 0 && resp.status().as_u16() == 206;
            // `bytes <start>-<end>/<total>`: the rest must start where the partial file ends.
            let start = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(|r| r.strip_prefix("bytes ")?.split('-').next()?.parse::<u64>().ok());
            if resumed && start != Some(from) {
                return Err(Fault::Unexpected { host: Fault::host(url) });
            }
            let reader: Box<dyn Read + Send> = Box::new(resp.into_body().into_reader());
            Ok(if resumed || from == 0 { Opened::From(reader) } else { Opened::Whole(reader) })
        }
        // The server lost track of the partial file: start over.
        Err(ureq::Error::StatusCode(416)) if from > 0 => open_source(url, 0).map(|o| match o {
            Opened::From(r) | Opened::Whole(r) => Opened::Whole(r),
        }),
        Err(e) => Err(request_fault(url, e)),
    }
}

/// A request's failure: refused (`blocked`), an error status, or why the server was out of reach.
pub(crate) fn request_fault(url: &str, err: ureq::Error) -> Fault {
    // The transport's own words, for the log a user can send.
    eprintln!("browser engine: {url}: {err}");
    let host = Fault::host(url);
    let why = match err {
        ureq::Error::StatusCode(status) if blocked(status) => return Fault::Blocked { host, status },
        ureq::Error::StatusCode(status) => return Fault::Server { host, status },
        ureq::Error::Io(e) => return io_fault(url, &e),
        ureq::Error::HostNotFound => Net::NotFound,
        ureq::Error::ConnectionFailed => Net::Refused,
        ureq::Error::Timeout(_) => Net::TimedOut,
        ureq::Error::Tls(_) | ureq::Error::Pem(_) | ureq::Error::Rustls(_) => Net::Secure,
        ureq::Error::ConnectProxyFailed(_) | ureq::Error::InvalidProxyUrl => Net::Proxy,
        _ => Net::Other,
    };
    Fault::Unreachable { host, why }
}

/// A socket's failure before any of the archive came.
pub(crate) fn io_fault(url: &str, err: &std::io::Error) -> Fault {
    use std::io::ErrorKind;
    // WSAHOST_NOT_FOUND, WSATRY_AGAIN, WSANO_DATA: the name didn't resolve.
    if cfg!(windows) && matches!(err.raw_os_error(), Some(11001 | 11002 | 11004)) {
        return Fault::Unreachable { host: Fault::host(url), why: Net::NotFound };
    }
    let why = match err.kind() {
        ErrorKind::ConnectionRefused => Net::Refused,
        ErrorKind::TimedOut => Net::TimedOut,
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable | ErrorKind::NetworkDown => Net::NotFound,
        _ => Net::Other,
    };
    Fault::Unreachable { host: Fault::host(url), why }
}
