//! The OS saying the network changed (or, on Windows, that the machine woke): Darwin posts a
//! notification when SystemConfiguration's network state changes; Windows calls back (below).
//! One reader for the process; a closed window doesn't retain its waker.
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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn subscribe(_: rune::Waker) -> Option<Arc<Change>> {
    None
}

/// Windows' notifications ring here; the one thread hears them (below).
#[cfg(target_os = "windows")]
static BELL: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
fn ring() {
    if let Some(tx) = BELL.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        tx.send(()).ok();
    }
}

/// `--harness-network-change <secs>` (the resilience harness): rings as the OS's notification
/// would, `secs` after launch, since a harness can't make Windows wake from sleep or change its
/// network. Nothing else differs from the real event.
pub(super) fn simulate() {
    #[cfg(target_os = "windows")]
    ring();
}

/// Windows: the network's reach changed (`NotifyNetworkConnectivityHintChange`: none, local,
/// internet, constrained), or the machine woke (`PowerRegisterSuspendResumeNotification`, the
/// window-less form of `WM_POWERBROADCAST`'s `PBT_APMRESUMEAUTOMATIC` / `PBT_APMRESUMESUSPEND`).
/// Both call back on the system's thread pool only when something happens. Not
/// `NotifyIpInterfaceChange`: it calls back for every parameter of every interface, and on a
/// machine with virtual adapters that was a wake every 1-2 s while idle (0.5-0.8 wakes/s
/// measured here, each waking every window); a reach change is the one that matters. Where the
/// hint isn't there (before Windows 10 2004) an interface added or removed stands in. A change can
/// come as a burst, so the one thread here wakes the windows once it has been quiet for 150 ms.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub(super) fn subscribe(wake: rune::Waker) -> Option<Arc<Change>> {
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock, Weak, mpsc};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{HANDLE, NO_ERROR};
    use std::sync::atomic::AtomicI32;
    use windows_sys::Win32::NetworkManagement::IpHelper::{MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, MibAddInstance, MibDeleteInstance, GetNetworkConnectivityHint, NotifyIpInterfaceChange, NotifyNetworkConnectivityHintChange};
    use windows_sys::Win32::Networking::WinSock::{AF_UNSPEC, NL_NETWORK_CONNECTIVITY_HINT};
    use windows_sys::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND};
    static WINDOWS: Mutex<Vec<Weak<Change>>> = Mutex::new(Vec::new());
    static WATCH: OnceLock<bool> = OnceLock::new();
    /// The reach last heard, so a hint that says the same again (its cost, say) is no change.
    static LEVEL: AtomicI32 = AtomicI32::new(-1);
    unsafe extern "system" fn reach_changed(_: *const c_void, hint: NL_NETWORK_CONNECTIVITY_HINT) {
        if LEVEL.swap(hint.ConnectivityLevel, Ordering::AcqRel) != hint.ConnectivityLevel {
            ring();
        }
    }
    unsafe extern "system" fn interface_changed(_: *const c_void, _: *const MIB_IPINTERFACE_ROW, kind: MIB_NOTIFICATION_TYPE) {
        if kind == MibAddInstance || kind == MibDeleteInstance {
            ring();
        }
    }
    unsafe extern "system" fn power_changed(_: *const c_void, kind: u32, _: *const c_void) -> u32 {
        if matches!(kind, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) {
            ring();
        }
        0
    }
    let started = WATCH.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<()>();
        *BELL.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        let thread = std::thread::Builder::new().name("network-change".into()).spawn(move || {
            while rx.recv().is_ok() {
                while rx.recv_timeout(Duration::from_millis(150)).is_ok() {}
                opencode_kit::client::Server::wake_reconnects();
                // No stream that looks up can be trusted now: each is checked, and replaced unless it
                // speaks (`client::streams`).
                opencode_kit::client::verify_streams();
                WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).retain(|w| {
                    let Some(w) = w.upgrade() else { return false };
                    w.changed.store(true, Ordering::Release);
                    (w._wake)();
                    true
                });
            }
        });
        if thread.is_err() {
            return false;
        }
        // Both registrations last the process's life: their handles are never closed.
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: `NL_NETWORK_CONNECTIVITY_HINT` is a plain C struct valid when zeroed.
        let mut now: NL_NETWORK_CONNECTIVITY_HINT = unsafe { std::mem::zeroed() };
        // SAFETY: `&mut now` is a valid, aligned out-pointer for `GetNetworkConnectivityHint`.
        if unsafe { GetNetworkConnectivityHint(&mut now) } == NO_ERROR {
            LEVEL.store(now.ConnectivityLevel, Ordering::Release);
        }
        // SAFETY: `reach_changed` is a plain `extern "system"` callback touching only `LEVEL` and `BELL`; `&mut handle` is a valid out-pointer kept for the process's life.
        let net = unsafe { NotifyNetworkConnectivityHintChange(Some(reach_changed), std::ptr::null(), false, &mut handle) } == NO_ERROR
            // SAFETY: `interface_changed` is a plain `extern "system"` callback that only rings `BELL`; `&mut handle` is a valid out-pointer kept for the process's life.
            || unsafe { NotifyIpInterfaceChange(AF_UNSPEC, Some(interface_changed), std::ptr::null(), false, &mut handle) } == NO_ERROR;
        let params: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(power_changed), Context: std::ptr::null_mut() }));
        let mut registration: *mut c_void = std::ptr::null_mut();
        // SAFETY: with DEVICE_NOTIFY_CALLBACK the recipient is the parameters, which live forever.
        let woke = unsafe { PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, std::ptr::from_mut(params).cast(), &mut registration) } == NO_ERROR;
        net || woke
    });
    if !started {
        return None;
    }
    let w = Arc::new(Change { changed: AtomicBool::new(false), _wake: wake });
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&w));
    Some(w)
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
                // SAFETY: `name` is a static NUL-terminated C string and `&mut fd` / `&mut token`
                // are valid writable `i32` out-pointers; libnotify owns `fd` until `notify_cancel`.
                if unsafe { notify_register_file_descriptor(c"com.apple.system.config.network_change".as_ptr(), &mut fd, 0, &mut token) } != 0 {
                    return;
                }
                if fd < 0 {
                    // SAFETY: `notify_register_file_descriptor` succeeded and initialized `token`.
                    unsafe { notify_cancel(token) };
                    return;
                }
                // SAFETY: `fd` is a valid non-negative descriptor opened by libnotify and kept live
                // until `notify_cancel(token)` below; `ManuallyDrop` ensures `File` never closes it.
                let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
                while file.read_exact(&mut [0; 4]).is_ok() {
                    opencode_kit::client::Server::wake_reconnects();
                    opencode_kit::client::verify_streams();
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
                // SAFETY: `token` was created by `notify_register_file_descriptor` above, `file`
                // (wrapped in `ManuallyDrop`) is no longer used, and `notify_cancel` closes `fd` once.
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
