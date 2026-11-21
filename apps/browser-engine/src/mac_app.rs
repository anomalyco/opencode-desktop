//! NSApplication subclass implementing CEF's `CrAppProtocol` / `CefAppProtocol` on macOS.

use std::cell::Cell;
use std::ffi::c_void;

use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol};
use objc2::{ClassType, DefinedClass, MainThreadMarker, define_class, extern_methods, msg_send};
use objc2_app_kit::{NSApp, NSApplication, NSEvent};

#[derive(Default)]
pub struct AppIvars {
    handling_send_event: Cell<Bool>,
}

define_class!(
    #[unsafe(super(NSApplication))]
    #[name = "OpenCodeCefApplication"]
    #[ivars = AppIvars]
    pub struct OpenCodeCefApplication;

    impl OpenCodeCefApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let prev = self.ivars().handling_send_event.get();
            self.ivars().handling_send_event.set(Bool::YES);
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            self.ivars().handling_send_event.set(prev);
        }
    }

    unsafe impl CrAppControlProtocol for OpenCodeCefApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling_send_event(&self, handling: Bool) {
            self.ivars().handling_send_event.set(handling);
        }
    }

    unsafe impl CrAppProtocol for OpenCodeCefApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn is_handling_send_event(&self) -> Bool {
            self.ivars().handling_send_event.get()
        }
    }

    unsafe impl CefAppProtocol for OpenCodeCefApplication {}
);

impl OpenCodeCefApplication {
    extern_methods! {
        #[unsafe(method(sharedApplication))]
        pub fn shared_application() -> Retained<Self>;
    }
}

pub fn ensure_ns_application() {
    let mtm = MainThreadMarker::new().expect("CEF must initialize on the main thread");
    let _ = OpenCodeCefApplication::shared_application();
    assert!(NSApp(mtm).isKindOfClass(OpenCodeCefApplication::class()));
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    fn CFDictionaryGetValue(the_dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFArrayGetCount(the_array: *const c_void) -> isize;
    fn CFStringGetLength(the_string: *const c_void) -> isize;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecCodeInfoTeamIdentifier: *const c_void;
    static kSecCodeInfoCertificates: *const c_void;
    fn SecCodeCopySelf(flags: u32, self_code: *mut *const c_void) -> i32;
    fn SecCodeCopySigningInformation(code: *const c_void, flags: u32, info: *mut *const c_void) -> i32;
}

/// Returns `true` iff the running process is code-signed with a real Apple certificate chain and
/// Team Identifier (e.g. Developer ID `5NZ4Q7NXJ4`), as opposed to unsigned or ad-hoc signed.
/// Used to gate real macOS Keychain access so ad-hoc local dev builds stay on `--use-mock-keychain`
/// instead of polluting `"OpenCode Browser Safe Storage"` with a changing per-build CDHash.
pub fn has_team_code_signature() -> bool {
    const K_SEC_CS_DEFAULT_FLAGS: u32 = 0;
    const K_SEC_CS_SIGNING_INFORMATION: u32 = 2;
    unsafe {
        let mut self_code: *const c_void = std::ptr::null();
        if SecCodeCopySelf(K_SEC_CS_DEFAULT_FLAGS, &mut self_code) != 0 || self_code.is_null() {
            return false;
        }
        let mut info: *const c_void = std::ptr::null();
        let status = SecCodeCopySigningInformation(self_code, K_SEC_CS_SIGNING_INFORMATION, &mut info);
        CFRelease(self_code);
        if status != 0 || info.is_null() {
            return false;
        }
        let team = CFDictionaryGetValue(info, kSecCodeInfoTeamIdentifier);
        let certs = CFDictionaryGetValue(info, kSecCodeInfoCertificates);
        let ok = !team.is_null()
            && CFStringGetLength(team) > 0
            && !certs.is_null()
            && CFArrayGetCount(certs) > 0;
        CFRelease(info);
        ok
    }
}

pub fn pump_run_loop(seconds: f64) {
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 1);
    }
}
