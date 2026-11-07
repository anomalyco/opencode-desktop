//! Loading a plugin library and the C ABI handshake.
//!
//! With hot reload on, the host loads a shadow copy (`<cache>/plugin-hot/<id>-<pid>-<n>.<ext>`),
//! so the file `cargo build` writes is never locked (Windows' `LNK1104`). A `LoadedLibrary` is
//! reference-counted by every callback and handler that points into it; the library unloads
//! only once the last one drops, so nothing can call into unmapped code.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use opencode_plugin::abi::*;

use super::vtable::{HOST_ALLOC, HOST_VTABLE};

/// How a plugin's code got into the process.
pub enum Code {
    /// A library on disk, unloaded when this drops.
    Dynamic { _library: libloading::Library },
    /// Linked into the app (`--features static-builtins`), entered through the same function.
    Static,
}

pub struct LoadedLibrary {
    code: Option<Code>,
    /// The shadow copy to delete once unloaded.
    shadow: Option<PathBuf>,
}

impl Drop for LoadedLibrary {
    fn drop(&mut self) {
        drop(self.code.take());
        if let Some(shadow) = &self.shadow {
            std::fs::remove_file(shadow).ok();
        }
    }
}

/// A library past the handshake.
pub struct Loaded {
    pub lib: Arc<LoadedLibrary>,
    pub vtable: PluginVtable,
}

/// Loads `path` (through a shadow copy in `shadow_dir` when given) and shakes hands. Safe off
/// the app's thread: it touches no host state, so the hot-reload watcher runs it.
pub fn load(path: &Path, shadow_dir: Option<&Path>, id: &str) -> Result<Loaded, String> {
    static COPIES: AtomicU32 = AtomicU32::new(0);
    if !path.is_file() {
        return Err(format!("library not found: {}", path.display()));
    }
    let shadow = match shadow_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).ok();
            let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
            let n = COPIES.fetch_add(1, Ordering::Relaxed) + 1;
            let copy = dir.join(format!("{id}-{}-{n}.{ext}", std::process::id()));
            std::fs::copy(path, &copy).map_err(|e| format!("couldn't copy the library for hot reload: {e}"))?;
            Some(copy)
        }
        None => None,
    };
    let open = shadow.as_deref().unwrap_or(path);
    let library = open_library(open).inspect_err(|_| {
        if let Some(copy) = &shadow {
            std::fs::remove_file(copy).ok();
        }
    })?;
    // SAFETY: every plugin library exports `opencode_plugin_v1` with this signature; a library that
    // doesn't fails the lookup.
    let entry: PluginEntryFn = unsafe {
        *library.get::<PluginEntryFn>(ENTRY_SYMBOL).map_err(|e| format!("not an OpenCode plugin library (no `opencode_plugin_v1`): {e}"))?
    };
    let lib = Arc::new(LoadedLibrary { code: Some(Code::Dynamic { _library: library }), shadow });
    shake(entry, lib)
}

/// Enters a plugin linked into the app.
pub fn load_static(entry: PluginEntryFn) -> Result<Loaded, String> {
    shake(entry, Arc::new(LoadedLibrary { code: Some(Code::Static), shadow: None }))
}

fn open_library(path: &Path) -> Result<libloading::Library, String> {
    #[cfg(windows)]
    // SAFETY: loading runs the library's initialisers; plugins are trusted code, as the design states.
    let library = unsafe {
        libloading::os::windows::Library::load_with_flags(path, libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH).map(Into::into)
    };
    #[cfg(unix)]
    // SAFETY: as above. `RTLD_LOCAL` keeps one plugin's symbols from interposing another's.
    let library = unsafe {
        libloading::os::unix::Library::open(Some(path), libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_LOCAL).map(Into::into)
    };
    library.map_err(|e| {
        let blocked = if super::motw::blocked(path) { " Windows blocked this file because it came from another computer; unblock it from Settings › Plugins." } else { "" };
        format!("couldn't load {}: {e}.{blocked}", path.display())
    })
}

fn shake(entry: PluginEntryFn, lib: Arc<LoadedLibrary>) -> Result<Loaded, String> {
    let handshake = HostHandshake {
        magic: ABI_MAGIC,
        endian: ABI_ENDIAN,
        abi_epoch: ABI_EPOCH,
        abi_minor: ABI_MINOR,
        abi_layout_hash: ABI_LAYOUT_HASH,
        pointer_width: size_of::<usize>() as u8,
        os: HOST_OS,
        arch: HOST_ARCH,
        _pad: 0,
        host_vtable_size: size_of::<HostApiVtable>() as u32,
        host_vtable: &HOST_VTABLE,
        host_alloc: &HOST_ALLOC,
    };
    // SAFETY: `entry` is the plugin's entry point; the handshake is valid for the call.
    let descriptor = unsafe { entry(&handshake) };
    // SAFETY: a plugin returns a pointer to its static descriptor (or null).
    let Some(d) = (unsafe { descriptor.as_ref() }) else { return Err("the plugin returned no descriptor".to_owned()) };
    if d.magic != ABI_MAGIC || d.endian != ABI_ENDIAN {
        return Err("not an OpenCode plugin library (bad magic or byte order)".to_owned());
    }
    // SAFETY: the descriptor's version string points at the plugin's static data, alive while loaded.
    let sdk = unsafe { d.sdk_version.as_str() }.to_owned();
    if d.abi_epoch != ABI_EPOCH {
        return Err(format!(
            "targets experimental ABI epoch {}, this app is epoch {ABI_EPOCH}; rebuild it against opencode-plugin {SDK_VERSION} (it used {sdk})",
            d.abi_epoch
        ));
    }
    if d.abi_layout_hash != ABI_LAYOUT_HASH {
        return Err(format!(
            "ABI layout hash {:016x} doesn't match this app's {ABI_LAYOUT_HASH:016x}; rebuild it against opencode-plugin {SDK_VERSION} (it used {sdk})",
            d.abi_layout_hash
        ));
    }
    if (d.plugin_vtable_size as usize) < size_of::<PluginVtable>() || d.plugin_vtable.is_null() {
        return Err("the plugin's vtable is smaller than this app needs".to_owned());
    }
    // Copied out: the host keeps no pointer into the library's read-only data.
    // SAFETY: checked non-null and at least as large as `PluginVtable`.
    let vtable = unsafe { *d.plugin_vtable };
    Ok(Loaded { lib, vtable })
}

/// Deletes shadow copies an earlier run left (a crash, or a copy still locked then). A copy
/// another running app has loaded stays locked, so it survives the sweep.
pub fn sweep(shadow_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(shadow_dir) else { return };
    for e in entries.filter_map(Result::ok) {
        std::fs::remove_file(e.path()).ok();
    }
}

/// The SHA-256 of the file at `path`, in hex: what a trust decision is about.
pub fn hash(path: &Path) -> Option<String> {
    use sha2::Digest;
    let bytes = std::fs::read(path).ok()?;
    Some(sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}
