//! What the OS says about background work: whether it should wait (a metered connection, battery
//! power) and how to run a thread at background priority.
//!
//! Windows reads both cheaply on each check: `GetSystemPowerStatus` for the power line and the
//! internet connection profile's cost (`NetworkInformation`) for metering. macOS and Linux don't
//! pause yet: their checks report nothing, so the download goes ahead at the background rate.

use crate::installer::PauseReason;

/// Why background work should wait now, if it should.
pub(crate) fn pause_reason() -> Option<PauseReason> {
    imp::pause_reason()
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

    pub(super) fn pause_reason() -> Option<PauseReason> {
        if on_battery() {
            return Some(PauseReason::OnBattery);
        }
        metered().then_some(PauseReason::Metered)
    }

    fn on_battery() -> bool {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS for the call to fill.
        let ok = unsafe { GetSystemPowerStatus(&mut status) }.is_ok();
        // ACLineStatus: 0 offline (on battery), 1 online, 255 unknown.
        ok && status.ACLineStatus == 0
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

    pub(super) fn pause_reason() -> Option<PauseReason> {
        None
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

    pub(super) fn pause_reason() -> Option<PauseReason> {
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
