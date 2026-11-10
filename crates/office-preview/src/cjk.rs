//! The Chinese, Japanese and Korean font pack, installed on first need rather than shipped: five
//! Noto faces, 27 MB. Each file is fetched from BetterOffice's repository at the commit the
//! engines are pinned to and kept only when its SHA-256 matches the one recorded here, so nothing
//! but those exact bytes is ever loaded.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The pack's version, the folder it installs into under the fonts directory.
pub const VERSION: &str = "0.4.2";

const BASE: &str = "https://raw.githubusercontent.com/openooxml/betteroffice/ed0a3cea5bd24119c4f2e144a978849177e06fcd/packages/fonts-cjk/assets";

/// Every file of the pack, its size and SHA-256, the manifest last so a complete folder is the
/// only one that reads as installed.
const FILES: &[(&str, u64, &str)] = &[
    ("NotoSansSC-Regular.otf.zst", 6_444_964, "77d3e2ce92f605b1f2bce135f069ab841a2e534718ddf02f0831f3f2c685be73"),
    ("NotoSansTC-Regular.otf.zst", 4_456_580, "ca029b7401b9f2c377d1f085f79ec5f3364cfc56e26691cc874f10bb5eb17bb6"),
    ("NotoSansJP-Regular.otf.zst", 3_595_531, "d57a65d832e8d4a2b3e77109e99ce29c5f6a51c00b201c848bf5c30c9a999253"),
    ("NotoSansKR-Regular.otf.zst", 3_420_953, "d402cd7d2b59d8059308c6b62ebbfcc88205f629c6ee471f05a33d9ef316bffb"),
    ("NotoSerifSC-Regular.otf.zst", 8_743_526, "423fcbae39396201b9f1df57e77bc10756ce7764750327ec05f2e47bd02b9d8b"),
    ("manifest.json", 2_619, "d22ab29f48d7a46bc77b90f4cacfc942b1a13e49124f0f75cfdd0dc74a7e3eba"),
];

/// Where the pack lives under `fonts`.
pub fn dir(fonts: &Path) -> PathBuf {
    fonts.join("cjk").join(VERSION)
}

/// Whether the pack is complete under `fonts`.
pub fn installed(fonts: &Path) -> bool {
    let dir = dir(fonts);
    FILES.iter().all(|(name, size, _)| std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size))
}

/// Fetches every missing file with `fetch(url)` and writes it under `fonts` once its hash
/// matches. Blocking: run it off the UI thread.
pub fn install(fonts: &Path, fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<PathBuf, String> {
    let dir = dir(fonts);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, size, sha) in FILES {
        let path = dir.join(name);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == *size) {
            continue;
        }
        let bytes = fetch(&format!("{BASE}/{name}"))?;
        let digest: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if bytes.len() as u64 != *size || digest != *sha {
            return Err(format!("{name} does not match the pinned font pack"));
        }
        let partial = path.with_extension("part");
        std::fs::write(&partial, &bytes).map_err(|e| format!("{}: {e}", partial.display()))?;
        std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(dir)
}
