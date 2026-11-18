//! Linux: the engine's lines, with the file descriptors it sends beside some of them
//! (`SCM_RIGHTS`, only with `EngineMsg::DmabufPool` lines). The engine sends a line with
//! descriptors in one `sendmsg`, attached to its first byte, and a read never takes two sends'
//! descriptors (the kernel stops after one), so a read's descriptors belong to the one pool line
//! that starts within it. They're kept with the stream range the read covered, handed to that
//! line (`take_fds`), and closed once reading has passed the range unclaimed.
#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::transport::UnixStream;

/// How much one read asks for: a BGRA paint's line is megabytes long.
const READ: usize = 256 * 1024;
/// Descriptors one read takes at most (a pool sends three).
const MAX_FDS: usize = 16;

pub(crate) struct FdLines {
    stream: UnixStream,
    buf: Vec<u8>,
    /// Where the next line starts in `buf`, and how far it has been searched for its end.
    start: usize,
    scanned: usize,
    /// The stream offset of `buf[0]`.
    base: u64,
    /// Each read's descriptors, with the stream range it covered.
    fds: VecDeque<(std::ops::Range<u64>, Vec<OwnedFd>)>,
}

impl FdLines {
    pub(crate) fn new(stream: UnixStream) -> Self {
        Self { stream, buf: Vec::new(), start: 0, scanned: 0, base: 0, fds: VecDeque::new() }
    }

    /// Appends the next line, with its newline, to `line`, as `BufRead::read_line` does (0 at the
    /// end), and returns its length and stream offset.
    pub(crate) fn read_line(&mut self, line: &mut String) -> io::Result<(usize, u64)> {
        loop {
            if let Some(i) = memchr::memchr(b'\n', &self.buf[self.scanned..]) {
                let end = self.scanned + i + 1;
                return self.take(line, end);
            }
            self.scanned = self.buf.len();
            if self.start > 0 {
                self.buf.drain(..self.start);
                self.base += self.start as u64;
                self.scanned -= self.start;
                self.start = 0;
            }
            if self.recv()? == 0 {
                let end = self.buf.len();
                return self.take(line, end);
            }
        }
    }

    /// The descriptors sent with the pool line at stream offset `at`.
    pub(crate) fn take_fds(&mut self, at: u64) -> Vec<OwnedFd> {
        match self.fds.iter().position(|(range, _)| range.contains(&at)) {
            Some(i) => self.fds.remove(i).map(|(_, fds)| fds).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    fn take(&mut self, line: &mut String, end: usize) -> io::Result<(usize, u64)> {
        let at = self.base + self.start as u64;
        // Reads that ended before this line are past: their unclaimed descriptors close.
        while self.fds.front().is_some_and(|(range, _)| range.end <= at) {
            self.fds.pop_front();
        }
        let bytes = &self.buf[self.start..end];
        line.push_str(std::str::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?);
        let n = end - self.start;
        self.start = end;
        self.scanned = end;
        Ok((n, at))
    }

    fn recv(&mut self) -> io::Result<usize> {
        self.buf.reserve(READ);
        let len = self.buf.len();
        let at = self.base + len as u64;
        let spare = self.buf.capacity() - len;
        // SAFETY: `len` is within the allocation; the spare capacity starts there.
        let mut iov = libc::iovec { iov_base: unsafe { self.buf.as_mut_ptr().add(len) }.cast(), iov_len: spare };
        // SAFETY: CMSG_SPACE is a pure size computation. The buffer is u64s, so its headers are
        // aligned as `cmsghdr` needs.
        let space = unsafe { libc::CMSG_SPACE((MAX_FDS * size_of::<i32>()) as u32) } as usize;
        let mut control = vec![0u64; space.div_ceil(8)];
        // SAFETY: an all-zero msghdr is valid; its pointers are set to buffers that outlive the call.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let n = loop {
            // SAFETY: the iovec points at the buffer's spare capacity, the control at `control`.
            let n = unsafe { libc::recvmsg(self.stream.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
            if n >= 0 {
                break n as usize;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        };
        // SAFETY: recvmsg wrote `n` bytes into the spare capacity.
        unsafe { self.buf.set_len(len + n) };
        // Descriptors cut off for want of room are lost: the line they came with gets too few,
        // which its reader treats as a broken set (`dmabuf.rs`).
        let mut fds = Vec::new();
        // SAFETY: walks the control headers recvmsg filled; each SCM_RIGHTS payload is an array
        // of descriptors this process now owns.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&msg);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(header).cast::<i32>();
                    let count = ((*header).cmsg_len as usize).saturating_sub(libc::CMSG_LEN(0) as usize) / size_of::<i32>();
                    for i in 0..count {
                        fds.push(OwnedFd::from_raw_fd(data.add(i).read_unaligned()));
                    }
                }
                header = libc::CMSG_NXTHDR(&msg, header);
            }
        }
        if !fds.is_empty() {
            self.fds.push_back((at..at + n as u64, fds));
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            eprintln!("browser: the engine sent more descriptors than one read takes; some were lost");
        }
        Ok(n)
    }
}
