//! Hello's Greeter service, as plugins that use it see it: a `#[repr(C)]` table of functions
//! over Hello's own data, published process-wide by Hello's app half under `hello.greeter`.
//!
//! A consumer depends on this crate (not on Hello) and reads the service through
//! `cx.consume(&GREETER)`. Every call runs Hello's code in Hello's library; the host keeps that
//! library loaded while the call runs, and a hot reload of Hello publishes the service again
//! with the next generation.
#![allow(unsafe_code)]

use core::ffi::c_void;

use opencode_plugin::Service;
use opencode_plugin::abi::{AbiOwnedBytes, AbiStr};

#[repr(C)]
pub struct Greeter {
    /// Hello's data behind the functions.
    pub this: *const c_void,
    /// Writes Hello's greeting for `name` into `out`.
    pub greet: unsafe extern "C" fn(this: *const c_void, name: AbiStr, out: *mut AbiOwnedBytes),
    /// How many times Hello's counter went up in this window.
    pub clicks: unsafe extern "C" fn(this: *const c_void) -> i64,
    /// Frees `this`; Hello's own code, run when the host replaces the service.
    pub release: Option<unsafe extern "C" fn(this: *const c_void)>,
}

// SAFETY: Hello's functions are thread-safe over data behind atomics.
unsafe impl Send for Greeter {}
// SAFETY: as above.
unsafe impl Sync for Greeter {}

impl Drop for Greeter {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            // SAFETY: frees `this` once, with the provider's own function.
            unsafe { release(self.this) };
        }
    }
}

impl Greeter {
    pub fn greet(&self, name: &str) -> String {
        let mut out = AbiOwnedBytes::EMPTY;
        // SAFETY: `this` is the provider's data, alive while the service is; `out` is valid.
        unsafe { (self.greet)(self.this, AbiStr::new(name), &mut out) };
        out.to_string_lossy()
    }

    pub fn clicks(&self) -> i64 {
        // SAFETY: as `greet`.
        unsafe { (self.clicks)(self.this) }
    }
}

/// Version 1 of the Greeter service.
pub const GREETER: Service<Greeter> = Service::new("hello.greeter", 1);
