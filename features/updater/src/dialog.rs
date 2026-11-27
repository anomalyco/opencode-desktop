//! Native OS message boxes for the app menu's Check for Updates… on non-beta channels
//! (`packages/gui-plugins/src/updater/main.ts:69-114`): error alert, up-to-date alert, and
//! Update Ready Restart/Later confirmation. Always called off the UI thread, and skipped in
//! headless runs.
#![cfg_attr(windows, allow(unsafe_code))]

/// Alert icon / severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Info,
    Error,
}

/// Shows a single-button native alert box (`OK`). No-op in headless builds or on `wasm32`.
pub fn alert(kind: Kind, title: &str, message: &str) {
    if rune::is_headless() {
        return;
    }
    #[cfg(windows)]
    win::alert(kind, title, message);
    #[cfg(target_os = "macos")]
    mac::alert(kind, title, message);
    #[cfg(target_os = "linux")]
    linux::alert(kind, title, message);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let _ = (kind, title, message);
}

/// Shows a two-button native confirmation dialog (`primary` default, `cancel` cancel) and
/// returns `true` when the user chose `primary`. Returns `false` in headless runs or on error.
pub fn confirm(title: &str, message: &str, primary: &str, cancel: &str) -> bool {
    if rune::is_headless() {
        return false;
    }
    #[cfg(windows)]
    {
        win::confirm(title, message, primary, cancel)
    }
    #[cfg(target_os = "macos")]
    {
        mac::confirm(title, message, primary, cancel)
    }
    #[cfg(target_os = "linux")]
    {
        linux::confirm(title, message, primary, cancel)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (title, message, primary, cancel);
        false
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::UI::Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_OK_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
        TD_ERROR_ICON, TD_INFORMATION_ICON, TaskDialogIndirect,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::IDCANCEL;

    use super::Kind;

    const PRIMARY_ID: i32 = 100;
    const CANCEL_ID: i32 = 101;

    fn wide(s: &str) -> Vec<u16> {
        s.chars().filter(|&c| c != '\0').collect::<String>().encode_utf16().chain(Some(0)).collect()
    }

    pub(super) fn alert(kind: Kind, title: &str, message: &str) {
        let title = wide(title);
        let content = wide(message);
        // SAFETY: a zeroed TASKDIALOGCONFIG is the documented default for every unset field.
        let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
        config.dwCommonButtons = TDCBF_OK_BUTTON;
        config.pszWindowTitle = title.as_ptr();
        config.Anonymous1.pszMainIcon = match kind {
            Kind::Info => TD_INFORMATION_ICON,
            Kind::Error => TD_ERROR_ICON,
        };
        config.pszContent = content.as_ptr();
        // SAFETY: title and content stay alive for the full duration of the modal call.
        unsafe { TaskDialogIndirect(&config, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
    }

    pub(super) fn confirm(title: &str, message: &str, primary: &str, cancel: &str) -> bool {
        let title = wide(title);
        let content = wide(message);
        let primary_w = wide(primary);
        let cancel_w = wide(cancel);
        let buttons = [
            TASKDIALOG_BUTTON { nButtonID: PRIMARY_ID, pszButtonText: primary_w.as_ptr() },
            TASKDIALOG_BUTTON { nButtonID: CANCEL_ID, pszButtonText: cancel_w.as_ptr() },
        ];
        // SAFETY: a zeroed TASKDIALOGCONFIG is the documented default for every unset field.
        let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
        config.pszWindowTitle = title.as_ptr();
        config.Anonymous1.pszMainIcon = TD_INFORMATION_ICON;
        config.pszContent = content.as_ptr();
        config.cButtons = buttons.len() as u32;
        config.pButtons = buttons.as_ptr();
        config.nDefaultButton = PRIMARY_ID;
        let mut chosen: i32 = IDCANCEL;
        // SAFETY: all strings and button descriptors outlive the modal call, which writes `chosen`.
        let hr = unsafe { TaskDialogIndirect(&config, &mut chosen, std::ptr::null_mut(), std::ptr::null_mut()) };
        hr == S_OK && chosen == PRIMARY_ID
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::process::{Command, Stdio};

    use super::Kind;

    pub(super) fn alert(kind: Kind, title: &str, message: &str) {
        let style = match kind {
            Kind::Info => "informational",
            Kind::Error => "critical",
        };
        let script = format!("on run argv\ndisplay alert (item 1 of argv) message (item 2 of argv) as {style}\nend run");
        Command::new("/usr/bin/osascript")
            .args(["-e", &script, "--", title, message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
    }

    pub(super) fn confirm(title: &str, message: &str, primary: &str, cancel: &str) -> bool {
        let script = "on run argv\nset r to display alert (item 1 of argv) message (item 2 of argv) as informational buttons {item 4 of argv, item 3 of argv} default button (item 3 of argv) cancel button (item 4 of argv)\nreturn button returned of r\nend run";
        let Ok(out) = Command::new("/usr/bin/osascript")
            .args(["-e", script, "--", title, message, primary, cancel])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return false;
        };
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == primary
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::process::{Command, Stdio};

    use super::Kind;

    fn system_bin(candidates: &[&'static str]) -> Option<&'static str> {
        candidates.iter().copied().find(|p| Path::new(p).is_file())
    }

    /// Escapes Qt rich-text / HTML metacharacters so `kdialog` never renders markup in `message`.
    fn escape_qt_html(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                '\0' => {}
                other => out.push(other),
            }
        }
        out
    }

    pub(super) fn alert(kind: Kind, title: &str, message: &str) {
        let flag = match kind {
            Kind::Info => "--info",
            Kind::Error => "--error",
        };
        if let Some(zenity) = system_bin(&["/usr/bin/zenity", "/bin/zenity"])
            && Command::new(zenity)
                .args([flag, "--title", title, "--text", message, "--no-markup"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        {
            return;
        }
        let Some(kdialog) = system_bin(&["/usr/bin/kdialog", "/bin/kdialog"]) else {
            return;
        };
        let kflag = match kind {
            Kind::Info => "--msgbox",
            Kind::Error => "--error",
        };
        let safe = escape_qt_html(message);
        Command::new(kdialog)
            .args(["--title", title, kflag, &safe])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
    }

    pub(super) fn confirm(title: &str, message: &str, primary: &str, cancel: &str) -> bool {
        if let Some(zenity) = system_bin(&["/usr/bin/zenity", "/bin/zenity"])
            && let Ok(st) = Command::new(zenity)
                .args(["--question", "--title", title, "--text", message, "--no-markup", "--ok-label", primary, "--cancel-label", cancel])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
        {
            return st.success();
        }
        let Some(kdialog) = system_bin(&["/usr/bin/kdialog", "/bin/kdialog"]) else {
            return false;
        };
        let safe = escape_qt_html(message);
        Command::new(kdialog)
            .args(["--title", title, "--yesno", &safe, "--yes-label", primary, "--no-label", cancel])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}
