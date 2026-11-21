//! External browser discovery, OS key-store unwrapping, Choose-sites host listing, and cookie
//! reading across all 15 Chromium/Firefox/Safari browsers (`profile/sources.ts`).
//!
//! Security properties:
//! - `read_source_hosts` and `read_source_cookies` only accept `(source_id, profile_id)` pairs
//!   returned by `list_sources()` (no caller-supplied filesystem paths).
//! - Windows unwraps Chromium `Local State` DPAPI blobs in-process via `CryptUnprotectData`
//!   (no shell, no PowerShell spawn).
//! - macOS reads Safe Storage passwords in-process via `security-framework` (`SecItemCopyMatching`).
//! - Linux invokes `/usr/bin/secret-tool` or `/bin/secret-tool` by absolute path with no shell.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use serde::Deserialize;

use super::chromium::{ChromiumKey, read_chromium_cookies, read_chromium_hosts};
use super::database::{SnapshotScratch, read_regular_bounded};
use super::firefox::{firefox_profiles, read_firefox_cookies, read_firefox_hosts};
use super::safari::parse_binary_cookies;
use super::sites::{group_sites, site_filter};
use super::{
    BrowserEngineKind, BrowserProfileInfo, BrowserSource, CookieHost, CookieRead, ImportFailure,
    MAX_COOKIE_FILE_BYTES, SiteList, SiteListOrder,
};

const MAX_LOCAL_STATE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PROFILES_INI_BYTES: u64 = 1024 * 1024;

struct Definition {
    id: &'static str,
    name: &'static str,
    engine: BrowserEngineKind,
    root: PathBuf,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    keychain: Option<(&'static str, &'static str)>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    keyring: &'static [&'static str],
    flat: bool,
}

struct LocatedProfile {
    id: String,
    name: String,
    directory: PathBuf,
    database: PathBuf,
    used_ms: u128,
}

fn user_home() -> PathBuf {
    #[cfg(not(windows))]
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(home);
    }
    #[cfg(windows)]
    if let Some(home) = std::env::var_os("USERPROFILE").filter(|s| !s.is_empty()) {
        return PathBuf::from(home);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn chromium_def(id: &'static str, name: &'static str, root: PathBuf) -> Definition {
    Definition {
        id,
        name,
        engine: BrowserEngineKind::Chromium,
        root,
        keychain: None,
        keyring: &[],
        flat: false,
    }
}

fn firefox_def(id: &'static str, name: &'static str, root: PathBuf) -> Definition {
    Definition {
        id,
        name,
        engine: BrowserEngineKind::Firefox,
        root,
        keychain: None,
        keyring: &[],
        flat: false,
    }
}

#[cfg(windows)]
fn definitions() -> Vec<Definition> {
    let home = user_home();
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Local"));
    let roaming = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Roaming"));

    let mut opera = chromium_def(
        "opera",
        "Opera",
        roaming.join("Opera Software").join("Opera Stable"),
    );
    opera.flat = true;

    vec![
        chromium_def("chrome", "Google Chrome", local.join("Google").join("Chrome").join("User Data")),
        chromium_def("edge", "Microsoft Edge", local.join("Microsoft").join("Edge").join("User Data")),
        chromium_def("helium", "Helium", local.join("imput").join("Helium").join("User Data")),
        chromium_def("brave", "Brave", local.join("BraveSoftware").join("Brave-Browser").join("User Data")),
        chromium_def(
            "arc",
            "Arc",
            local
                .join("Packages")
                .join("TheBrowserCompany.Arc_ttt1ap7aakyb4")
                .join("LocalCache")
                .join("Local")
                .join("Arc")
                .join("User Data"),
        ),
        chromium_def("vivaldi", "Vivaldi", local.join("Vivaldi").join("User Data")),
        opera,
        chromium_def("chromium", "Chromium", local.join("Chromium").join("User Data")),
        firefox_def("firefox", "Firefox", roaming.join("Mozilla").join("Firefox")),
        firefox_def("zen", "Zen", roaming.join("zen")),
        firefox_def("librewolf", "LibreWolf", roaming.join("librewolf")),
        firefox_def("floorp", "Floorp", roaming.join("Floorp")),
    ]
}

#[cfg(target_os = "macos")]
fn definitions() -> Vec<Definition> {
    let home = user_home();
    let support = home.join("Library").join("Application Support");
    let kc = |id: &'static str,
              name: &'static str,
              root: PathBuf,
              account: &'static str,
              service: &'static str,
              flat: bool| Definition {
        id,
        name,
        engine: BrowserEngineKind::Chromium,
        root,
        keychain: Some((service, account)),
        keyring: &[],
        flat,
    };

    vec![
        kc("chrome", "Google Chrome", support.join("Google").join("Chrome"), "Chrome", "Chrome Safe Storage", false),
        kc("edge", "Microsoft Edge", support.join("Microsoft Edge"), "Microsoft Edge", "Microsoft Edge Safe Storage", false),
        kc("helium", "Helium", support.join("net.imput.helium"), "Helium", "Helium Storage Key", false),
        kc("arc", "Arc", support.join("Arc").join("User Data"), "Arc", "Arc Safe Storage", false),
        kc("brave", "Brave", support.join("BraveSoftware").join("Brave-Browser"), "Brave", "Brave Safe Storage", false),
        kc("dia", "Dia", support.join("Dia").join("User Data"), "Dia", "Dia Safe Storage", false),
        kc("comet", "Comet", support.join("Comet"), "Comet", "Comet Safe Storage", false),
        kc("vivaldi", "Vivaldi", support.join("Vivaldi"), "Vivaldi", "Vivaldi Safe Storage", false),
        kc("opera", "Opera", support.join("com.operasoftware.Opera"), "Opera", "Opera Safe Storage", true),
        kc("chromium", "Chromium", support.join("Chromium"), "Chromium", "Chromium Safe Storage", false),
        Definition {
            id: "safari",
            name: "Safari",
            engine: BrowserEngineKind::Safari,
            root: home.join("Library"),
            keychain: None,
            keyring: &[],
            flat: false,
        },
        firefox_def("firefox", "Firefox", support.join("Firefox")),
        firefox_def("zen", "Zen", support.join("zen")),
        firefox_def("librewolf", "LibreWolf", support.join("librewolf")),
        firefox_def("floorp", "Floorp", support.join("Floorp")),
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn definitions() -> Vec<Definition> {
    let home = user_home();
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let kr = |id: &'static str,
              name: &'static str,
              root: PathBuf,
              keyring: &'static [&'static str],
              flat: bool| Definition {
        id,
        name,
        engine: BrowserEngineKind::Chromium,
        root,
        keychain: None,
        keyring,
        flat,
    };

    vec![
        kr("chrome", "Google Chrome", config.join("google-chrome"), &["chrome"], false),
        kr("edge", "Microsoft Edge", config.join("microsoft-edge"), &["chromium"], false),
        kr("helium", "Helium", config.join("net.imput.helium"), &["helium", "chromium"], false),
        kr("brave", "Brave", config.join("BraveSoftware").join("Brave-Browser"), &["brave"], false),
        kr("vivaldi", "Vivaldi", config.join("vivaldi"), &["vivaldi", "chrome"], false),
        kr("opera", "Opera", config.join("opera"), &["chromium"], true),
        kr("chromium", "Chromium", config.join("chromium"), &["chromium"], false),
        firefox_def("firefox", "Firefox", home.join(".mozilla").join("firefox")),
        firefox_def(
            "firefox-snap",
            "Firefox (Snap)",
            home.join("snap")
                .join("firefox")
                .join("common")
                .join(".mozilla")
                .join("firefox"),
        ),
        firefox_def("zen", "Zen", home.join(".zen")),
        firefox_def("librewolf", "LibreWolf", home.join(".librewolf")),
        firefox_def("floorp", "Floorp", home.join(".floorp")),
    ]
}

/// Lists installed external browsers with at least one profile holding cookies, ordered by most
/// recently modified cookie database first (`sources.ts::listSources`).
pub fn list_sources() -> Vec<BrowserSource> {
    let mut found: Vec<(u128, BrowserSource)> = Vec::new();
    for def in definitions() {
        let mut profiles = locate_profiles(&def);
        if profiles.is_empty() {
            continue;
        }
        profiles.sort_by_key(|a| Reverse(a.used_ms));
        let newest = profiles[0].used_ms;
        found.push((
            newest,
            BrowserSource {
                id: def.id.to_owned(),
                name: def.name.to_owned(),
                engine: def.engine,
                profiles: profiles
                    .into_iter()
                    .map(|p| BrowserProfileInfo {
                        id: p.id,
                        name: p.name,
                    })
                    .collect(),
            },
        ));
    }
    found.sort_by_key(|a| Reverse(a.0));
    found.into_iter().map(|(_, s)| s).collect()
}

/// Lists the registrable sites a browser profile has cookies for (without reading or decrypting any
/// cookie name/value, so no DPAPI/Keychain prompt is triggered; `sources.ts::readSourceHosts`).
pub fn list_source_sites(
    source_id: &str,
    profile_id: &str,
    scratch: &SnapshotScratch,
    now_secs: f64,
) -> SiteList {
    let hosts = match read_source_hosts(source_id, profile_id, scratch) {
        Ok(h) => h,
        Err(reason) => {
            let reason = if reason == ImportFailure::Key {
                ImportFailure::Read
            } else {
                reason
            };
            return SiteList::Failed { reason };
        }
    };
    let order = if hosts.iter().any(|h| h.used > 0.0) {
        SiteListOrder::Recent
    } else {
        SiteListOrder::Name
    };
    SiteList::Listed {
        sites: group_sites(&hosts, now_secs),
        order,
    }
}

/// Reads plain-text cookie hosts & timestamps for a validated `(source_id, profile_id)`.
pub fn read_source_hosts(
    source_id: &str,
    profile_id: &str,
    scratch: &SnapshotScratch,
) -> Result<Vec<CookieHost>, ImportFailure> {
    let (def, target) = locate_one(source_id, profile_id).ok_or(ImportFailure::Read)?;
    match def.engine {
        BrowserEngineKind::Firefox => read_firefox_hosts(&target.directory, scratch),
        BrowserEngineKind::Safari => {
            let data = read_regular_bounded(&target.database, MAX_COOKIE_FILE_BYTES)?;
            let read = parse_binary_cookies(&data, false).ok_or(ImportFailure::Read)?;
            Ok(read
                .cookies
                .into_iter()
                .map(|c| CookieHost {
                    host: c.host,
                    expires: c.expires,
                    used: 0.0,
                })
                .collect())
        }
        BrowserEngineKind::Chromium => read_chromium_hosts(&target.database, scratch),
    }
}

/// Reads and decrypts cookies from a validated `(source_id, profile_id)`, optionally restricted to
/// `chosen_sites` from `list_source_sites` (`sources.ts::readSource`).
pub fn read_source_cookies(
    source_id: &str,
    profile_id: &str,
    scratch: &SnapshotScratch,
    chosen_sites: Option<&[String]>,
) -> Result<CookieRead, ImportFailure> {
    let (def, target) = locate_one(source_id, profile_id).ok_or(ImportFailure::Read)?;
    match def.engine {
        BrowserEngineKind::Firefox => match chosen_sites {
            Some(sites) => read_firefox_cookies(&target.directory, scratch, Some(site_filter(sites))),
            None => read_firefox_cookies::<fn(&str) -> bool>(&target.directory, scratch, None),
        },
        BrowserEngineKind::Safari => {
            let data = read_regular_bounded(&target.database, MAX_COOKIE_FILE_BYTES)?;
            let mut read = parse_binary_cookies(&data, true).ok_or(ImportFailure::Read)?;
            if let Some(sites) = chosen_sites {
                let mut keep = site_filter(sites);
                read.cookies.retain(|c| keep(&c.host));
            }
            Ok(read)
        }
        BrowserEngineKind::Chromium => {
            let key = chromium_key(&def).ok_or(ImportFailure::Key)?;
            match chosen_sites {
                Some(sites) => {
                    read_chromium_cookies(&target.database, &key, scratch, Some(site_filter(sites)))
                }
                None => read_chromium_cookies::<fn(&str) -> bool>(
                    &target.database,
                    &key,
                    scratch,
                    None,
                ),
            }
        }
    }
}

fn locate_one(source_id: &str, profile_id: &str) -> Option<(Definition, LocatedProfile)> {
    let def = definitions().into_iter().find(|d| d.id == source_id)?;
    let target = locate_profiles(&def)
        .into_iter()
        .find(|p| p.id == profile_id)?;
    Some((def, target))
}

fn locate_profiles(def: &Definition) -> Vec<LocatedProfile> {
    match def.engine {
        BrowserEngineKind::Safari => {
            let candidates = [
                def.root
                    .join("Containers")
                    .join("com.apple.Safari")
                    .join("Data")
                    .join("Library")
                    .join("Cookies")
                    .join("Cookies.binarycookies"),
                def.root.join("Cookies").join("Cookies.binarycookies"),
            ];
            let Some(file) = candidates.into_iter().find(|p| p.is_file()) else {
                return Vec::new();
            };
            let used_ms = modified_ms(&file);
            vec![LocatedProfile {
                id: "default".to_owned(),
                name: "Default".to_owned(),
                directory: file.clone(),
                database: file,
                used_ms,
            }]
        }
        BrowserEngineKind::Firefox => {
            let Ok(bytes) = read_regular_bounded(&def.root.join("profiles.ini"), MAX_PROFILES_INI_BYTES) else {
                return Vec::new();
            };
            let Ok(ini) = String::from_utf8(bytes) else {
                return Vec::new();
            };
            firefox_profiles(&def.root, &ini)
                .into_iter()
                .filter_map(|p| {
                    let db = p.directory.join("cookies.sqlite");
                    db.is_file().then(|| LocatedProfile {
                        used_ms: modified_ms(&db),
                        id: p.id,
                        name: p.name,
                        directory: p.directory,
                        database: db,
                    })
                })
                .collect()
        }
        BrowserEngineKind::Chromium => {
            if def.flat {
                let Some(db) = chromium_cookie_db(&def.root) else {
                    return Vec::new();
                };
                return vec![LocatedProfile {
                    used_ms: modified_ms(&db),
                    id: "default".to_owned(),
                    name: "Default".to_owned(),
                    directory: def.root.clone(),
                    database: db,
                }];
            }
            local_state_profiles(&def.root)
                .into_iter()
                .filter_map(|(id, name)| {
                    let dir = def.root.join(&id);
                    let db = chromium_cookie_db(&dir)?;
                    Some(LocatedProfile {
                        used_ms: modified_ms(&db),
                        id,
                        name,
                        directory: dir,
                        database: db,
                    })
                })
                .collect()
        }
    }
}

fn chromium_cookie_db(dir: &Path) -> Option<PathBuf> {
    let network = dir.join("Network").join("Cookies");
    if network.is_file() {
        return Some(network);
    }
    let flat = dir.join("Cookies");
    if flat.is_file() {
        return Some(flat);
    }
    None
}

fn modified_ms(db: &Path) -> u128 {
    let mut wal = db.as_os_str().to_os_string();
    wal.push("-wal");
    [db.to_path_buf(), PathBuf::from(wal)]
        .into_iter()
        .filter_map(|p| {
            fs::metadata(p)
                .ok()?
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis())
        })
        .max()
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct LocalState {
    #[serde(default)]
    profile: Option<LocalStateProfile>,
    #[serde(default)]
    os_crypt: Option<LocalStateOsCrypt>,
}

#[derive(Deserialize)]
struct LocalStateProfile {
    #[serde(default)]
    info_cache: BTreeMap<String, LocalStateProfileInfo>,
}

#[derive(Deserialize)]
struct LocalStateProfileInfo {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct LocalStateOsCrypt {
    #[serde(default)]
    encrypted_key: Option<String>,
}

fn read_local_state(root: &Path) -> Option<LocalState> {
    let bytes = read_regular_bounded(&root.join("Local State"), MAX_LOCAL_STATE_BYTES).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn local_state_profiles(root: &Path) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    if let Some(state) = read_local_state(root)
        && let Some(profile) = state.profile
    {
        for (id, info) in profile.info_cache {
            // Only accept plain single-component directory names (`sources.ts:424`).
            let plain = !id.is_empty()
                && id != "."
                && id != ".."
                && !id.contains('/')
                && !id.contains('\\');
            if plain {
                let display = info.name.filter(|s| !s.is_empty()).unwrap_or_else(|| id.clone());
                names.insert(id, display);
            }
        }
    }
    names.entry("Default".to_owned()).or_insert_with(|| "Default".to_owned());
    names
}

fn zeroize_bytes(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        *b = 0;
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(windows)]
fn chromium_key(def: &Definition) -> Option<ChromiumKey> {
    let encoded = read_local_state(&def.root)?.os_crypt?.encrypted_key?;
    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let payload = wrapped.strip_prefix(b"DPAPI")?;
    let mut raw = dpapi_unprotect(payload)?;
    let key: Option<[u8; 32]> = raw.as_slice().try_into().ok();
    zeroize_bytes(&mut raw);
    key.map(ChromiumKey::Gcm)
}

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn dpapi_unprotect(blob: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};

    let len = u32::try_from(blob.len()).ok()?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: len,
        pbData: blob.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points to valid read-only `blob` bytes for the duration of the synchronous
    // Win32 call; `CryptUnprotectData` allocates `output.pbData` with `LocalAlloc` on success,
    // which we copy, zeroize in place, and unconditionally release via `LocalFree` below.
    let ok = unsafe { CryptUnprotectData(&input, None, None, None, None, 0, &mut output) };
    if ok.is_err() || output.pbData.is_null() {
        return None;
    }
    // SAFETY: `CryptUnprotectData` succeeded and returned `output.cbData` valid mutable bytes at `pbData`.
    let out_slice = unsafe { std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let bytes = out_slice.to_vec();
    zeroize_bytes(out_slice);
    // SAFETY: `output.pbData` was allocated by `CryptUnprotectData` and is freed once here.
    let _ = unsafe { LocalFree(Some(HLOCAL(output.pbData.cast()))) };
    Some(bytes)
}

#[cfg(target_os = "macos")]
fn chromium_key(def: &Definition) -> Option<ChromiumKey> {
    use super::chromium::derive_cbc_key;

    let (service, account) = def.keychain?;
    let mut password = security_framework::passwords::get_generic_password(service, account).ok()?;
    let trimmed_len = trim_trailing_newlines(&password).len();
    let v10 = derive_cbc_key(&password[..trimmed_len], 1003);
    zeroize_bytes(&mut password);
    Some(ChromiumKey::Cbc { v10, v11: None })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn chromium_key(def: &Definition) -> Option<ChromiumKey> {
    use std::process::{Command, Stdio};

    use super::chromium::derive_cbc_key;

    let tool = ["/usr/bin/secret-tool", "/bin/secret-tool"]
        .into_iter()
        .find(|p| Path::new(p).is_file());
    let v11 = tool.and_then(|bin| {
        let bin_dir = Path::new(bin).parent()?;
        def.keyring.iter().find_map(|app| {
            let mut out = Command::new(bin)
                .current_dir(bin_dir)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .args(["lookup", "application", app])
                .output()
                .ok()?;
            if !out.status.success() {
                zeroize_bytes(&mut out.stdout);
                return None;
            }
            let len = trim_trailing_newlines(&out.stdout).len();
            let key = (len > 0).then(|| derive_cbc_key(&out.stdout[..len], 1));
            zeroize_bytes(&mut out.stdout);
            key
        })
    });
    Some(ChromiumKey::Cbc {
        v10: derive_cbc_key(b"peanuts", 1),
        v11,
    })
}

#[cfg(not(windows))]
fn trim_trailing_newlines(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &bytes[..end]
}
