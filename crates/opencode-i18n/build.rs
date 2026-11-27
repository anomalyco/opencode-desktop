use std::path::{Path, PathBuf};

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    println!("cargo:rustc-check-cfg=cfg(bundle_assets)");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUNDLE_ASSETS");
    let bundled = std::env::var("OPENCODE_RUNE_BUNDLE_ASSETS").as_deref() == Ok("1") && std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if !bundled {
        return rune_i18n::codegen::generate(Path::new("locales"), &out);
    }
    // The packaged macOS app (`apps/desktop/scripts/build-app.sh`) keeps English in the exe and
    // every other language as two files in `Contents/Resources/i18n` (`src/bundle.rs`): macOS
    // reads every byte of a new exe before its first run, and 40 languages are 7.5 MiB of it.
    // The files go beside the exe (`target/[<triple>/]<profile>/i18n`), where the script takes them.
    println!("cargo:rustc-cfg=bundle_assets");
    rune_i18n::codegen::generate_later(Path::new("locales"), &out)?;
    let Some(profile) = out.ancestors().nth(3) else { return Ok(()) };
    let beside = profile.join("i18n");
    std::fs::create_dir_all(&beside)?;
    for entry in std::fs::read_dir(out.join("i18n"))? {
        let entry = entry?;
        std::fs::copy(entry.path(), beside.join(entry.file_name()))?;
    }
    Ok(())
}