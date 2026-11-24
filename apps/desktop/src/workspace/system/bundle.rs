//! Files the macOS app carries in `Contents/Resources` in place of bytes inside the exe: the
//! fonts and the sound clips. macOS reads every byte of a new executable file before its first
//! run (XProtect: 5.6-5.9 ms per MiB on an M4), on every install and update; a file beside the
//! exe is read when the app asks for it and no sooner. `build-app.sh` builds with
//! `OPENCODE_RUNE_BUNDLE_ASSETS=1` (`cfg(bundle_assets)`) and copies the files; every other
//! build (`cargo run`, `bundle.sh`, Windows, Linux, the web) keeps them in the exe.

use std::path::PathBuf;
use std::sync::OnceLock;

/// `name` under the running bundle's `Contents/Resources`; `None` outside a bundle.
pub fn path(name: &str) -> Option<PathBuf> {
    static RESOURCES: OnceLock<Option<PathBuf>> = OnceLock::new();
    let resources = RESOURCES.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let resources = exe.parent()?.parent()?.join("Resources");
        resources.is_dir().then_some(resources)
    });
    Some(resources.as_ref()?.join(name))
}

/// The bundle's file `name`, mapped for the life of the process: its pages come from the file
/// as they are read, as the exe's did. `None` when the file is missing or empty.
#[allow(unsafe_code)]
pub fn map(name: &str) -> Option<&'static [u8]> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path(name)?).ok()?;
    let len = usize::try_from(file.metadata().ok()?.len()).ok().filter(|len| *len > 0)?;
    // SAFETY: a private read-only mapping of a whole open file; the kernel picks the address,
    // and the mapping is never unmapped, so the slice is valid for the life of the process. The
    // bundle's files are replaced as a whole bundle (an update swaps the `.app`), never written
    // in place.
    let at = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_PRIVATE, file.as_raw_fd(), 0) };
    if at == libc::MAP_FAILED {
        return None;
    }
    // SAFETY: `at` is the start of `len` readable bytes that stay mapped (above).
    Some(unsafe { std::slice::from_raw_parts(at.cast::<u8>(), len) })
}
