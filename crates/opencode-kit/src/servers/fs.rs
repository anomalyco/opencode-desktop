//! The remote folder picker's reads: a server's directories through `/api/fs`, for a server
//! whose files aren't this computer's (WSL, SSH, another machine), where the native folder
//! dialog can't reach (`workspaces/selection/domain.ts:404-417`, `picker.tsx:29-76`). Blocking,
//! so off the app's thread only.

use std::time::Duration;

use serde_json::Value;

use super::probe::{RequestError, get_json};
use super::registry::HttpBase;

/// What an entry is (`FileSystemEntry.type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    File,
    Directory,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Directory => "directory",
        }
    }
}

/// One entry, with its absolute path on the server (`listPickerDirectory`'s items).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub absolute: String,
}

/// A listing and the directory the server resolved its location to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Listing {
    pub location: String,
    pub entries: Vec<Entry>,
}

/// How long one listing may take before the picker gives up on it.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// `GET /api/fs/list` (`client.file.list`): `path`'s entries, relative to `location` (the
/// server's own directory when None).
pub fn list(http: &HttpBase, location: Option<&str>, path: Option<&str>, timeout: Duration) -> Result<Listing, RequestError> {
    let query = [location.map(|d| ("location%5Bdirectory%5D", d)), path.map(|p| ("path", p))];
    listing(get_json(http, &with_query("/api/fs/list", &query), timeout)?)
}

/// `GET /api/fs/find` (`client.file.find`): entries matching `query` under `location`.
pub fn find(http: &HttpBase, location: Option<&str>, query: &str, kind: Option<Kind>, limit: Option<u32>, timeout: Duration) -> Result<Listing, RequestError> {
    let limit = limit.map(|l| l.to_string());
    let params = [location.map(|d| ("location%5Bdirectory%5D", d)), Some(("query", query)), kind.map(|k| ("type", k.name())), limit.as_deref().map(|l| ("limit", l))];
    listing(get_json(http, &with_query("/api/fs/find", &params), timeout)?)
}

fn with_query(path: &str, params: &[Option<(&str, &str)>]) -> String {
    let q: Vec<String> = params.iter().flatten().map(|(k, v)| format!("{k}={}", crate::client::encode_component(v))).collect();
    if q.is_empty() { path.to_owned() } else { format!("{path}?{}", q.join("&")) }
}

/// `{ location: { directory }, data: [{ path, type }] }`, each path made absolute.
fn listing(v: Value) -> Result<Listing, RequestError> {
    let location = v["location"]["directory"].as_str().ok_or_else(|| RequestError::Body("no location".into()))?.to_owned();
    let entries = v["data"]
        .as_array()
        .ok_or_else(|| RequestError::Body("no data".into()))?
        .iter()
        .filter_map(|e| {
            let kind = match e["type"].as_str()? {
                "directory" => Kind::Directory,
                "file" => Kind::File,
                _ => return None,
            };
            let absolute = absolute_path(e["path"].as_str()?, &location);
            Some(Entry { name: crate::path::basename(&absolute).to_owned(), kind, absolute })
        })
        .collect();
    Ok(Listing { location, entries })
}

/// `pickerAbsolutePath` (`domain.ts:77-82`): `input` as is when rooted, else under `current`,
/// with `.` and `..` resolved and forward slashes.
pub fn absolute_path(input: &str, current: &str) -> String {
    let value = normalize_drive(input);
    let absolute = if root(&value).is_empty() { join(current, &value) } else { value };
    canonical(&absolute)
}

/// `canonicalPickerPath` (`domain.ts:88-107`).
pub fn canonical(path: &str) -> String {
    let value = normalize_drive(path);
    let root = root(&value);
    let mut out: Vec<&str> = Vec::new();
    for part in value[root.len()..].split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    join(&root, &out.join("/"))
}

/// `normalizePickerPath` (`domain.ts:278-284`): forward slashes, runs of them collapsed except a
/// UNC path's leading pair.
fn normalize(input: &str) -> String {
    let value = input.replace('\\', "/");
    let collapse = |s: &str| {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if !(c == '/' && out.ends_with('/')) {
                out.push(c);
            }
        }
        out
    };
    if value.starts_with("//") && !value.starts_with("///") {
        return format!("//{}", collapse(&value[2..]));
    }
    collapse(&value)
}

/// `normalizePickerDrive` (`domain.ts:286-292`): a bare drive gets its root slash.
fn normalize_drive(input: &str) -> String {
    let value = normalize(input);
    if is_drive(&value) { format!("{value}/") } else { value }
}

fn is_drive(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// `trimPickerPath` (`domain.ts:294-300`).
fn trim(input: &str) -> String {
    let value = normalize_drive(input);
    let b = value.as_bytes();
    if value == "/" || value == "//" || (b.len() == 3 && is_drive(&value[..2]) && b[2] == b'/') {
        return value;
    }
    value.trim_end_matches('/').to_owned()
}

/// `joinPickerPath` (`domain.ts:302-313`).
fn join(base: &str, relative: &str) -> String {
    let root = trim(base);
    let path = trim(relative);
    let path = path.trim_start_matches('/');
    if root.is_empty() {
        return path.to_owned();
    }
    if path.is_empty() {
        return root;
    }
    if root.ends_with('/') { format!("{root}{path}") } else { format!("{root}/{path}") }
}

/// `pickerRoot` (`domain.ts:315-330`): `//server/share`, `/`, `C:/`, or none.
fn root(input: &str) -> String {
    let value = normalize_drive(input);
    if let Some(rest) = value.strip_prefix("//") {
        let mut parts = rest.split('/');
        return match (parts.next(), parts.next()) {
            (Some(server), Some(share)) if !server.is_empty() && !share.is_empty() => format!("//{server}/{share}"),
            _ => "//".to_owned(),
        };
    }
    if value.starts_with('/') {
        return "/".to_owned();
    }
    let b = value.as_bytes();
    if b.len() >= 3 && is_drive(&value[..2]) && b[2] == b'/' {
        return value[..3].to_owned();
    }
    String::new()
}
