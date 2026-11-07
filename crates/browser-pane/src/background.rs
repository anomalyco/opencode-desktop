//! What the OS says about background work: whether it should wait (a metered connection, battery
//! power) and how to run a thread at background priority.
//!
//! Windows reads both cheaply on each check: `GetSystemPowerStatus` for the power line and the
//! internet connection profile's cost (`NetworkInformation`) for metering. macOS and Linux don't
//! pause yet: their checks report nothing, so the download goes ahead (throttled, at low priority).

use crate::installer::PauseReason;

/// Why background work should wait now, if it should.
pub(crate) fn pause_reason() -> Option<PauseReason> {
    imp::pause_reason()
}

/// Runs the calling thread at background priority (`true`), or back at normal priority.
pub(crate) fn set_background_priority(background: bool) {
    imp::set_background_priority(background);
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
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), mode) };
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use crate::installer::PauseReason;

    pub(super) fn pause_reason() -> Option<PauseReason> {
        None
    }

    pub(super) fn set_background_priority(background: bool) {
        let class = if background {
            libc::qos_class_t::QOS_CLASS_BACKGROUND
        } else {
            libc::qos_class_t::QOS_CLASS_USER_INITIATED
        };
        // SAFETY: sets the calling thread's own QoS class; no pointers are involved.
        let _ = unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use crate::installer::PauseReason;

    pub(super) fn pause_reason() -> Option<PauseReason> {
        None
    }

    pub(super) fn set_background_priority(_background: bool) {}
}
