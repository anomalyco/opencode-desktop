//! Session exports saved to the Downloads folder.

use serde_json::Value;

/// `sessionExportFilename`: the title (or slug, or id) lowercased, runs of anything but
/// letters, digits, `_` and `-` as one `-`, trimmed of `-`, plus `.json`.
pub fn export_filename(info: &Value, id: &str) -> String {
    let name = ["title", "slug"].iter().filter_map(|k| info[*k].as_str()).find(|s| !s.is_empty()).unwrap_or(id);
    let mut clean = String::new();
    let mut replacing = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            clean.push(c);
            replacing = false;
        } else if !replacing {
            clean.push('-');
            replacing = true;
        }
        if clean.len() == 250 { break; }
    }
    let clean = clean.trim_matches('-');
    format!("{}.json", if clean.is_empty() { id } else { clean })
}

static DOWNLOADS: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Where ordinary file downloads go, set once at startup. Session export uses a save dialog.
pub fn set_download_dir(dir: Option<std::path::PathBuf>) {
    DOWNLOADS.set(dir).ok();
}

/// Writes `json` as `name` in the Downloads folder [`set_download_dir`] named; returns its path.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_download(name: &str, json: &str) -> Result<String, String> {
    let dir = DOWNLOADS.get().cloned().flatten().ok_or("No Downloads folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// [`save_download`] in a browser: the page downloads it, where the browser puts downloads.
#[cfg(target_arch = "wasm32")]
pub fn save_download(name: &str, json: &str) -> Result<String, String> {
    use wasm_bindgen::JsCast;
    let failed = |_| "The browser refused the download".to_owned();
    let parts = js_sys::Array::of1(&json.into());
    let options = web_sys::BlobPropertyBag::new();
    options.set_type("application/json");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options).map_err(failed)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(failed)?;
    let document = web_sys::window().and_then(|w| w.document()).ok_or("No page")?;
    let link: web_sys::HtmlAnchorElement = document.create_element("a").map_err(failed)?.unchecked_into();
    link.set_href(&url);
    link.set_download(name);
    link.click();
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(name.to_owned())
}
