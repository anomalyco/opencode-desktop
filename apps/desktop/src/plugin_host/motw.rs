//! "Blocked by Windows": a library downloaded from the internet carries the Mark of the Web, an
//! NTFS `Zone.Identifier` stream naming the zone it came from. Windows may refuse to load it, or
//! warn before it runs, so the trust prompt says so and offers to unblock it (as Explorer's
//! Properties › Unblock does: delete the stream). Elsewhere there is no such mark.

use std::path::Path;

/// Whether `path` came from the Internet (zone 3) or a restricted site (zone 4).
#[cfg(windows)]
pub fn blocked(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(stream(path)) else { return false };
    text.lines().filter_map(|l| l.trim().strip_prefix("ZoneId=")).any(|z| z.trim().parse::<u32>().is_ok_and(|z| z >= 3))
}

/// Removes the mark, as Unblock does.
#[cfg(windows)]
pub fn unblock(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(stream(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("couldn't unblock {}: {e}", path.display())),
    }
}

#[cfg(windows)]
fn stream(path: &Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(":Zone.Identifier");
    s.into()
}

#[cfg(not(windows))]
pub fn blocked(_: &Path) -> bool {
    false
}

#[cfg(not(windows))]
pub fn unblock(_: &Path) -> Result<(), String> {
    Ok(())
}
