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
}

pub fn pump_run_loop(seconds: f64) {
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 1);
    }
}
