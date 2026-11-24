//! A language's messages from the macOS bundle (`Contents/Resources/i18n/<id>.txt` and
//! `<id>.spans`), for the packaged app, whose exe carries English only (`build.rs`). The files are
//! the build's own blobs, so there is nothing to parse: they are read once, when the language is
//! first chosen, and kept for the life of the process.

/// Whether locale `index` (`id`) has its messages: here already, or read from the bundle now.
/// A bundle without the file leaves the app in the language it shows.
pub fn load(index: usize, id: &str) -> bool {
    if crate::CATALOG.present(index) {
        return true;
    }
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.join("Resources").join("i18n"))) else { return false };
    let (text, spans) = match (std::fs::read_to_string(dir.join(format!("{id}.txt"))), std::fs::read(dir.join(format!("{id}.spans")))) {
        (Ok(text), Ok(spans)) => (text, spans),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("i18n: no messages for {id} in {}: {e}", dir.display());
            return false;
        }
    };
    // The catalog hands out `&'static str`s into the text.
    match crate::CATALOG.load_blob(index, Box::leak(text.into_boxed_str()), Box::leak(spans.into_boxed_slice())) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("i18n: {id}: {e}");
            false
        }
    }
}