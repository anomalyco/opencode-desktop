//! The OS cursor for a widget holding the pointer captured (`widgets-input.md`): where it is on
//! screen, and putting it back there. Rune hides the cursor but has no pointer lock or raw
//! mouse motion, so the widget keeps the cursor where it was granted by moving it back there as
//! it drifts, and reads relative motion from the moves in between. macOS (CoreGraphics) and
//! Windows; elsewhere the cursor only hides.

/// The cursor's place on screen, in the OS's global coordinates.
pub fn at() -> Option<[f64; 2]> {
    os::at()
}

/// Puts the cursor back at `to` (from [`at`]) without a move event.
pub fn warp(to: [f64; 2]) {
    os::warp(to);
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod os {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CGWarpMouseCursorPosition(to: CGPoint) -> i32;
        fn CGAssociateMouseAndMouseCursorPosition(connected: u8) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(object: *const c_void);
    }

    pub fn at() -> Option<[f64; 2]> {
        // SAFETY: a null source makes a fresh event whose location is the cursor's; it is
        // released once read.
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let p = CGEventGetLocation(event);
            CFRelease(event);
            Some([p.x, p.y])
        }
    }

    pub fn warp(to: [f64; 2]) {
        // SAFETY: plain value calls. Re-associating right after the warp lifts the quarter
        // second AppKit otherwise holds mouse moves back after one.
        unsafe {
            CGWarpMouseCursorPosition(CGPoint { x: to[0], y: to[1] });
            CGAssociateMouseAndMouseCursorPosition(1);
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod os {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

    pub fn at() -> Option<[f64; 2]> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: `p` is a valid out pointer.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some([f64::from(p.x), f64::from(p.y)])
    }

    pub fn warp(to: [f64; 2]) {
        // SAFETY: plain value call.
        unsafe {
            SetCursorPos(to[0].round() as i32, to[1].round() as i32);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod os {
    pub fn at() -> Option<[f64; 2]> {
        None
    }

    pub fn warp(_to: [f64; 2]) {}
}
