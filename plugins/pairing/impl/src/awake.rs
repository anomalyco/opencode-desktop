//! Keep screen active (`powerSaveBlocker.start("prevent-display-sleep")`): Windows' execution
//! state on the app's thread, which lasts until cleared or the app ends; elsewhere a supervised
//! `caffeinate -d` (macOS) or `systemd-inhibit` (Linux) for as long as it's held, which the host
//! kills when the plugin unloads or the app exits.

use opencode_plugin::prelude::*;

#[derive(Default)]
pub struct Blocker {
    held: bool,
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    process: Option<Process>,
}

impl Blocker {
    pub fn held(&self) -> bool {
        self.held
    }

    /// Holds or releases the display; false when the OS refused.
    pub fn hold(&mut self, cx: &mut ActionCx, on: bool) -> bool {
        #[cfg(target_os = "windows")]
        let ok = {
            let _ = cx;
            windows::hold(on)
        };
        #[cfg(not(target_os = "windows"))]
        let ok = {
            if let Some(p) = self.process.take() {
                cx.kill_process(p);
            }
            if on {
                self.process = cx.spawn_process::<crate::Pairing>(&inhibitor(), |_, cx, _| cx.unchanged());
                self.process.is_some()
            } else {
                true
            }
        };
        if ok {
            self.held = on;
        }
        ok
    }

    /// Holds the display from launch, the setting being on.
    pub fn hold_at_setup(&mut self, cx: &mut WindowSetupCx<crate::Pairing>) {
        #[cfg(target_os = "windows")]
        {
            let _ = cx;
            self.held = windows::hold(true);
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.process = cx.spawn_process::<crate::Pairing>(&inhibitor(), |_, cx, _| cx.unchanged());
            self.held = self.process.is_some();
        }
    }
}

impl Drop for Blocker {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        if self.held {
            windows::hold(false);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn inhibitor() -> ProcessSpec {
    if cfg!(target_os = "macos") {
        ProcessSpec::new("caffeinate").args(["-d", "-w", &std::process::id().to_string()])
    } else {
        ProcessSpec::new("systemd-inhibit").args(["--what=idle", "--who=OpenCode", "--why=Keep screen active", "sleep", "infinity"])
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    /// The app's thread holds it (every plugin callback runs there).
    pub fn hold(on: bool) -> bool {
        // SAFETY: plain flags in, the previous state out (0 when it failed).
        unsafe { SetThreadExecutionState(if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS }) != 0 }
    }
}
