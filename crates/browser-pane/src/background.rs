//! What the OS says about background work: whether it should wait (a metered connection, the
//! computer saving energy, battery power) and how to run a thread at background priority.
//!
//! Windows reads them cheaply on each check: `GetSystemPowerStatus` for the power line and the
//! battery saver, and the internet connection profile's cost (`NetworkInformation`) for metering.
//! macOS asks the network path whether it is expensive (a phone's hotspot, cellular) or
//! constrained (Low Data Mode), through one path monitor that tells of each change, and the
//! process whether Low Power Mode is on. Linux doesn't pause yet: its check reports nothing.

use crate::installer::PauseReason;

/// Why background work should wait now, if it should: a metered connection or the computer
/// saving energy, and for an `update` (an engine already runs, so it can wait longer) battery
/// power too.
pub(crate) fn pause_reason(update: bool) -> Option<PauseReason> {
    imp::pause_reason(update)
}

/// Runs the calling thread at background priority (`true`), or back at normal priority.
pub(crate) fn set_background_priority(background: bool) {
    imp::set_background_priority(background);
}

/// Bytes free for this user on the volume holding `dir` (which must exist).
pub(crate) fn free_space(dir: &std::path::Path) -> Option<u64> {
    imp::free_space(dir)
}

/// Whether process `pid` still runs (true when it can't tell).
pub(crate) fn pid_alive(pid: u32) -> bool {
    imp::pid_alive(pid)
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use windows::Networking::Connectivity::{NetworkCostType, NetworkInformation};
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_MODE_BACKGROUND_END,
    };

    use crate::installer::PauseReason;

    pub(super) fn pause_reason(update: bool) -> Option<PauseReason> {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS for the call to fill.
        if unsafe { GetSystemPowerStatus(&mut status) }.is_ok() {
            // SystemStatusFlag: 1 while the battery saver is on.
            if status.SystemStatusFlag == 1 {
                return Some(PauseReason::BatterySaver);
            }
            // ACLineStatus: 0 offline (on battery), 1 online, 255 unknown.
            if update && status.ACLineStatus == 0 {
                return Some(PauseReason::OnBattery);
            }
        }
        metered().then_some(PauseReason::Metered)
    }

    /// The internet connection costs per byte, roams, or is near or over its data limit. No
    /// connection (or no answer) isn't metered: the download then fails and retries on its own.
    fn metered() -> bool {
        let cost = NetworkInformation::GetInternetConnectionProfile().and_then(|p| p.GetConnectionCost());
        let Ok(cost) = cost else { return false };
        let kind = cost.NetworkCostType().unwrap_or(NetworkCostType::Unknown);
        matches!(kind, NetworkCostType::Fixed | NetworkCostType::Variable)
            || cost.Roaming().unwrap_or(false)
            || cost.OverDataLimit().unwrap_or(false)
            || cost.ApproachingDataLimit().unwrap_or(false)
    }

    pub(super) fn set_background_priority(background: bool) {
        let mode = if background { THREAD_MODE_BACKGROUND_BEGIN } else { THREAD_MODE_BACKGROUND_END };
        // SAFETY: the pseudo-handle names the calling thread; background mode also lowers its
        // I/O and memory priority, and only the thread itself may enter or leave it.
        unsafe { SetThreadPriority(GetCurrentThread(), mode) }.ok();
    }

    pub(super) fn pid_alive(pid: u32) -> bool {
        use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: the handle is closed before returning; `code` is ours to fill.
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
            let mut code = 0u32;
            let running = GetExitCodeProcess(process, &mut code).is_err() || code == STILL_ACTIVE.0 as u32;
            CloseHandle(process).ok();
            running
        }
    }

    pub(super) fn free_space(dir: &std::path::Path) -> Option<u64> {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        // SAFETY: `wide` is a NUL-terminated path that outlives the call; `free` is ours to fill.
        unsafe { windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
        Some(free)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[allow(unsafe_code)]
mod imp {
    use crate::installer::PauseReason;

    #[cfg(target_os = "linux")]
    pub(super) fn pause_reason(_update: bool) -> Option<PauseReason> {
        None
    }

    #[cfg(target_os = "macos")]
    pub(super) fn pause_reason(_update: bool) -> Option<PauseReason> {
        if mac::low_power() {
            return Some(PauseReason::BatterySaver);
        }
        mac::metered().then_some(PauseReason::Metered)
    }

    /// macOS: the network path's cost (Network.framework's path monitor, whose handler is a
    /// block) and the process's Low Power Mode (Foundation), both through their C entry points.
    #[cfg(target_os = "macos")]
    mod mac {
        use std::ffi::{c_char, c_int, c_void};
        use std::sync::Once;
        use std::sync::atomic::{AtomicU8, Ordering};
        use std::time::Duration;

        /// The path monitor told of a path; it is expensive; it is constrained.
        const SEEN: u8 = 1;
        const EXPENSIVE: u8 = 2;
        const CONSTRAINED: u8 = 4;
        /// The current path, as the monitor last told it.
        static PATH: AtomicU8 = AtomicU8::new(0);
        static WATCH: Once = Once::new();
        /// How long the first ask waits for the monitor's first answer (it comes within a few ms).
        const FIRST_ANSWER: Duration = Duration::from_millis(250);

        /// An Objective-C block with no captured state: the layout the block ABI fixes.
        #[repr(C)]
        struct Block {
            isa: *const c_void,
            flags: c_int,
            reserved: c_int,
            invoke: unsafe extern "C" fn(*const Block, *mut c_void),
            descriptor: *const BlockDescriptor,
        }

        #[repr(C)]
        struct BlockDescriptor {
            reserved: usize,
            size: usize,
        }

        static DESCRIPTOR: BlockDescriptor = BlockDescriptor { reserved: 0, size: std::mem::size_of::<Block>() };
        /// `BLOCK_IS_GLOBAL`: copying and releasing the block do nothing.
        const BLOCK_IS_GLOBAL: c_int = 1 << 28;

        #[link(name = "Network", kind = "framework")]
        unsafe extern "C" {
            fn nw_path_monitor_create() -> *mut c_void;
            fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *mut c_void);
            fn nw_path_monitor_set_update_handler(monitor: *mut c_void, handler: *const Block);
            fn nw_path_monitor_start(monitor: *mut c_void);
            fn nw_path_is_expensive(path: *mut c_void) -> bool;
            fn nw_path_is_constrained(path: *mut c_void) -> bool;
        }

        #[link(name = "Foundation", kind = "framework")]
        #[link(name = "objc")]
        unsafe extern "C" {
            static _NSConcreteGlobalBlock: [*const c_void; 32];
            fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
            fn objc_getClass(name: *const c_char) -> *mut c_void;
            fn sel_registerName(name: *const c_char) -> *mut c_void;
            fn objc_msgSend();
        }

        unsafe extern "C" fn path_changed(_: *const Block, path: *mut c_void) {
            let mut bits = SEEN;
            // SAFETY: `path` is the live path the monitor hands its handler for this call.
            if unsafe { nw_path_is_expensive(path) } {
                bits |= EXPENSIVE;
            }
            // SAFETY: as above.
            if unsafe { nw_path_is_constrained(path) } {
                bits |= CONSTRAINED;
            }
            PATH.store(bits, Ordering::Release);
        }

        /// Whether the connection costs per byte (cellular, a personal hotspot) or the user asked
        /// it to carry less (Low Data Mode). No connection isn't metered: the download then fails
        /// and waits for the network on its own.
        pub(super) fn metered() -> bool {
            WATCH.call_once(|| {
                // The block and the monitor last the process's life: one handler call per change
                // of path, no thread of ours.
                let block: &'static Block = Box::leak(Box::new(Block {
                    // SAFETY: only the address of the runtime's class object for global blocks is taken.
                    isa: unsafe { (&raw const _NSConcreteGlobalBlock).cast() },
                    flags: BLOCK_IS_GLOBAL,
                    reserved: 0,
                    invoke: path_changed,
                    descriptor: &raw const DESCRIPTOR,
                }));
                // SAFETY: the monitor is created, given a system queue (QOS_CLASS_UTILITY, 0x11)
                // and a block that lives forever, and started; none of the calls keeps a pointer
                // of ours beyond those.
                unsafe {
                    let monitor = nw_path_monitor_create();
                    if monitor.is_null() {
                        return;
                    }
                    nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(0x11, 0));
                    nw_path_monitor_set_update_handler(monitor, block);
                    nw_path_monitor_start(monitor);
                }
                let asked = rune::time::Instant::now();
                while PATH.load(Ordering::Acquire) & SEEN == 0 && asked.elapsed() < FIRST_ANSWER {
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
            PATH.load(Ordering::Acquire) & (EXPENSIVE | CONSTRAINED) != 0
        }

        /// `NSProcessInfo.processInfo.isLowPowerModeEnabled` (macOS 12 and later; the app needs 13).
        pub(super) fn low_power() -> bool {
            // SAFETY: `objc_msgSend` is called through the signatures of the two messages sent:
            // `+[NSProcessInfo processInfo]` (no arguments, returns an object that lives as long
            // as the process) and `-isLowPowerModeEnabled` (no arguments, returns a BOOL, one
            // byte). The class and selector names are NUL-terminated literals.
            unsafe {
                let class = objc_getClass(c"NSProcessInfo".as_ptr());
                if class.is_null() {
                    return false;
                }
                type Send<R> = unsafe extern "C" fn(*mut c_void, *mut c_void) -> R;
                let object = std::mem::transmute::<unsafe extern "C" fn(), Send<*mut c_void>>(objc_msgSend);
                let info = object(class, sel_registerName(c"processInfo".as_ptr()));
                if info.is_null() {
                    return false;
                }
                let flag = std::mem::transmute::<unsafe extern "C" fn(), Send<i8>>(objc_msgSend);
                flag(info, sel_registerName(c"isLowPowerModeEnabled".as_ptr())) != 0
            }
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn set_background_priority(_background: bool) {}

    #[cfg(target_os = "macos")]
    pub(super) fn set_background_priority(background: bool) {
        let class = if background {
            libc::qos_class_t::QOS_CLASS_BACKGROUND
        } else {
            libc::qos_class_t::QOS_CLASS_USER_INITIATED
        };
        // SAFETY: sets the calling thread's own QoS class; no pointers are involved.
        let _ = unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
    }

    pub(super) fn pid_alive(pid: u32) -> bool {
        // SAFETY: signal 0 only asks whether the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) }
    }

    pub(super) fn free_space(dir: &std::path::Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: `libc::statvfs` is a C POD struct whose all-zero bit pattern is valid; `statvfs` initializes it below.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `stat` is ours to fill.
        (unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0).then(|| stat.f_bavail as u64 * stat.f_frsize as u64)
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use crate::installer::PauseReason;

    pub(super) fn pause_reason(_update: bool) -> Option<PauseReason> {
        None
    }

    pub(super) fn set_background_priority(_background: bool) {}

    pub(super) fn free_space(_dir: &std::path::Path) -> Option<u64> {
        None
    }

    pub(super) fn pid_alive(_pid: u32) -> bool {
        true
    }
}
