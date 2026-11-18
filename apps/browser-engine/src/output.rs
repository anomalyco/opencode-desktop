//! Reliable control messages and one replaceable software paint per tab. A paint notification
//! occupies its place in the same FIFO as control messages; replacement never adds queue entries.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use base64::Engine as _;
use crate::wire::EngineMsg;

/// A file descriptor sent with a message (`SCM_RIGHTS`); none off Unix.
#[cfg(unix)]
pub type Fd = std::os::fd::OwnedFd;
#[cfg(not(unix))]
pub type Fd = std::convert::Infallible;

pub struct Paint {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

enum Entry {
    Message(Box<EngineMsg>, Vec<Fd>),
    Paint(u64),
}

#[derive(Clone)]
pub struct Output {
    entries: mpsc::Sender<Entry>,
    pending: Arc<Mutex<HashMap<u64, Paint>>>,
}

impl Output {
    pub fn channel() -> (Self, impl Iterator<Item = (EngineMsg, Vec<Fd>)>) {
        let (entries, rx) = mpsc::channel();
        let pending = Arc::new(Mutex::new(HashMap::<u64, Paint>::new()));
        let output = Self { entries, pending: pending.clone() };
        let messages = rx.into_iter().filter_map(move |entry| match entry {
            Entry::Message(msg, fds) => Some((*msg, fds)),
            Entry::Paint(tab_id) => {
                let paint = pending.lock().unwrap().remove(&tab_id)?;
                Some((
                    EngineMsg::CpuFrame {
                        tab_id,
                        frame_id: paint.frame_id,
                        width: paint.width,
                        height: paint.height,
                        bgra_b64: base64::engine::general_purpose::STANDARD.encode(paint.bgra),
                    },
                    Vec::new(),
                ))
            }
        });
        (output, messages)
    }

    pub fn send(&self, msg: EngineMsg) -> Result<(), ()> {
        self.send_with(msg, Vec::new())
    }

    /// `msg` with file descriptors the app receives alongside it.
    pub fn send_with(&self, msg: EngineMsg, fds: Vec<Fd>) -> Result<(), ()> {
        if let EngineMsg::TabClosed { tab_id } = &msg {
            self.pending.lock().unwrap().remove(tab_id);
        }
        self.entries.send(Entry::Message(Box::new(msg), fds)).map_err(|_| ())
    }

    pub fn paint(&self, tab_id: u64, paint: Paint) {
        if self.pending.lock().unwrap().insert(tab_id, paint).is_none() {
            let _ = self.entries.send(Entry::Paint(tab_id));
        }
    }
}

/// Writes `line` to `stream`, with `fds` attached to its first byte, so the reader gets them no
/// later than the line.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn write_with_fds(stream: &std::os::unix::net::UnixStream, line: &[u8], fds: &[Fd]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::fd::AsRawFd;
    if fds.is_empty() {
        return (&*stream).write_all(line);
    }
    let raw: Vec<i32> = fds.iter().map(AsRawFd::as_raw_fd).collect();
    let bytes = std::mem::size_of_val(raw.as_slice());
    // SAFETY: CMSG_SPACE/CMSG_LEN are pure size computations. The buffer is u64s, so its headers
    // are aligned as `cmsghdr` needs.
    let space = unsafe { libc::CMSG_SPACE(bytes as u32) } as usize;
    let mut control = vec![0u64; space.div_ceil(8)];
    let mut iov = libc::iovec { iov_base: line.as_ptr() as *mut _, iov_len: line.len() };
    // SAFETY: an all-zero msghdr is valid; its pointers are set below to buffers that outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the control buffer is CMSG_SPACE(bytes) long, so its first header and data fit.
    let sent = unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(bytes as u32) as _;
        std::ptr::copy_nonoverlapping(raw.as_ptr().cast::<u8>(), libc::CMSG_DATA(header), bytes);
        libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL)
    };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    (&*stream).write_all(&line[sent as usize..])
}
