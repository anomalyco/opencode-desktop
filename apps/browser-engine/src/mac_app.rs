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
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        num_bytes: isize,
        encoding: u32,
        is_external_representation: u8,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopySelf(flags: u32, self_code: *mut *const c_void) -> i32;
    fn SecRequirementCreateWithString(text: *const c_void, flags: u32, requirement: *mut *const c_void) -> i32;
    fn SecCodeCheckValidity(code: *const c_void, flags: u32, requirement: *const c_void) -> i32;
}

/// A Developer ID Application signature from Anomaly's team: Apple's Developer ID intermediate
/// (`1.2.840.113635.100.6.2.6`) and leaf (`1.2.840.113635.100.6.1.13`) marker extensions, and
/// team `5NZ4Q7NXJ4` (the shape `apps/desktop/update-requirement.txt` pins for the app), on the
/// engine's own bundle identifier (`scripts/package-release.sh`), so no other Anomaly binary
/// passes for it.
const DEVELOPER_ID_REQUIREMENT: &str = "anchor apple generic and identifier \"co.anomaly.opencode.browser-engine\" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"5NZ4Q7NXJ4\"";

/// Whether the running engine is a valid Developer ID build signed by Anomaly's team
/// (`SecCodeCheckValidity` against [`DEVELOPER_ID_REQUIREMENT`]). Only such a build opens the real
/// Keychain item `"OpenCode Browser Safe Storage"`, whose access list names that signature. Ad-hoc,
/// "Apple Development", other-team or self-signed builds, and a signed build whose signature no
/// longer validates, would make macOS ask the person for their password (`SecurityAgent`) or
/// re-ACL the item, so they stay on `--use-mock-keychain` (READER-9 #378 should-fix 1).
pub fn is_developer_id_signed() -> bool {
    const K_SEC_CS_DEFAULT_FLAGS: u32 = 0;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    unsafe {
        let text = CFStringCreateWithBytes(
            std::ptr::null(),
            DEVELOPER_ID_REQUIREMENT.as_ptr(),
            DEVELOPER_ID_REQUIREMENT.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
            0,
        );
        if text.is_null() {
            return false;
        }
        let mut requirement: *const c_void = std::ptr::null();
        let created = SecRequirementCreateWithString(text, K_SEC_CS_DEFAULT_FLAGS, &mut requirement);
        CFRelease(text);
        if created != 0 || requirement.is_null() {
            return false;
        }
        let mut self_code: *const c_void = std::ptr::null();
        if SecCodeCopySelf(K_SEC_CS_DEFAULT_FLAGS, &mut self_code) != 0 || self_code.is_null() {
            CFRelease(requirement);
            return false;
        }
        let status = SecCodeCheckValidity(self_code, K_SEC_CS_DEFAULT_FLAGS, requirement);
        CFRelease(self_code);
        CFRelease(requirement);
        status == 0
    }
}

pub fn pump_run_loop(seconds: f64) {
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 1);
    }
}
