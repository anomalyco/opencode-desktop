//! Session exports saved to the Downloads folder.

use serde_json::Value;

/// `sessionExportFilename`: the title (or slug, or id) lowercased, runs of anything but
/// letters, digits, `_` and `-` as one `-`, trimmed of `-`, plus `.json`.
pub fn export_filename(info: &Value, id: &str) -> String {
    let name = ["title", "slug"].iter().filter_map(|k| info[*k].as_str()).find(|s| !s.is_empty()).unwrap_or(id);
    let mut clean = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            clean.push(c);
        } else if !clean.ends_with('-') {
            clean.push('-');
        }
    }
    let clean = clean.trim_matches('-');
    format!("{}.json", if clean.is_empty() { id } else { clean })
}

static DOWNLOADS: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Where [`Write::Export`] saves sessions: the app's Downloads folder, set once at startup.
pub fn set_download_dir(dir: Option<std::path::PathBuf>) {
    let _ = DOWNLOADS.set(dir);
}

/// Writes `json` as `name` in the Downloads folder [`set_download_dir`] named; returns its path.
pub fn save_download(name: &str, json: &str) -> Result<String, String> {
    let dir = DOWNLOADS.get().cloned().flatten().ok_or("No Downloads folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}
