//! Windows: the app doesn't pass the Program Compatibility Assistant's layers on to what it starts.
//!
//! A process an installer started, or one started by something such a process started, has
//! `__COMPAT_LAYER=DetectorsAppHealth` (and `Installer`) in its environment, and every process
//! created with that variable runs with ntdll's shim engine: `apphelp.dll` is loaded and hooks
//! each module's imports before `main` (`LdrpInitShimEngine`, `SE_DllLoaded`). On this app that
//! is 6,320 µs of a start (`--version`: 18,454 µs with the variable, 12,134 µs without); a
//! process the Start menu starts has neither. The variable is inherited, so without this the
//! update helper, the new version it starts, the CLI server and every terminal carry it for as
//! long as the chain of relaunches lasts.
//!
//! Only the assistant's own two layers go: a layer the user set on the app (Properties,
//! Compatibility) stays for its children, as Windows means it to.

/// Drops the assistant's layers from this process's environment. First thing in `main`.
pub fn shed() {
    let Some(layers) = std::env::var_os("__COMPAT_LAYER") else { return };
    let layers = layers.to_string_lossy();
    let kept: Vec<&str> = layers.split_whitespace().filter(|layer| !matches!(*layer, "DetectorsAppHealth" | "Installer")).collect();
    if kept.len() == layers.split_whitespace().count() {
        return;
    }
    // This process itself started with the shim engine: a launch trace says so.
    rune::marks::note("compat.layer", "inherited");
    // SAFETY: the first thing `main` does, before any other thread exists.
    #[allow(unsafe_code)]
    unsafe {
        if kept.is_empty() { std::env::remove_var("__COMPAT_LAYER") } else { std::env::set_var("__COMPAT_LAYER", kept.join(" ")) }
    }
}
