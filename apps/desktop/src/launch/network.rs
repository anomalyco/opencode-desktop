//! Darwin posts this notification when SystemConfiguration's network state changes.
//! One blocking reader for the process; a closed window doesn't retain its waker.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Change {
    changed: AtomicBool,
    _wake: rune::Waker,
}
impl Change {
    pub(super) fn take(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn subscribe(_: rune::Waker) -> Option<Arc<Change>> {
    None
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(super) fn subscribe(wake: rune::Waker) -> Option<Arc<Change>> {
    use std::{
        io::Read,
        os::fd::FromRawFd,
        sync::{Mutex, OnceLock, Weak},
    };
    static WINDOWS: Mutex<Vec<Weak<Change>>> = Mutex::new(Vec::new());
    static WATCH: OnceLock<bool> = OnceLock::new();
    unsafe extern "C" {
        fn notify_register_file_descriptor(name: *const std::ffi::c_char, fd: *mut i32, flags: i32, token: *mut i32) -> u32;
        fn notify_cancel(token: i32) -> u32;
    }
    let started = WATCH.get_or_init(|| {
        std::thread::Builder::new()
            .name("network-change".into())
            .spawn(|| {
                let (mut fd, mut token) = (-1, 0);
                // Both out-pointers are valid until registration returns. libnotify owns fd until cancel.
                if unsafe { notify_register_file_descriptor(c"com.apple.system.config.network_change".as_ptr(), &mut fd, 0, &mut token) } != 0 {
                    return;
                }
                let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
                while file.read_exact(&mut [0; 4]).is_ok() {
                    opencode_kit::client::Server::wake_reconnects();
                    WINDOWS.lock().unwrap().retain(|w| {
                        if let Some(w) = w.upgrade() {
                            w.changed.store(true, Ordering::Release);
                            (w._wake)();
                            true
                        } else {
                            false
                        }
                    });
                }
                unsafe { notify_cancel(token) };
            })
            .is_ok()
    });
    if !started {
        return None;
    }
    let w = Arc::new(Change { changed: AtomicBool::new(false), _wake: wake });
    WINDOWS.lock().unwrap().push(Arc::downgrade(&w));
    Some(w)
}
