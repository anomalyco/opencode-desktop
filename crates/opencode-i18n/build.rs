use std::path::{Path, PathBuf};

fn main() -> std::io::Result<()> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    rune_i18n::codegen::generate(Path::new("locales"), &out)
}
