//! The macOS updater: checks the release feed, downloads and verifies a signed bundle into
//! `~/Library/Caches/anomaly.opencode-rune/Updates`, and swaps it in on Restart to update.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rune::Waker;
use sha2::{Digest as _, Sha256};

use super::mac::{self, BundleInfo, TEAM_BUNDLE_ID};
use super::protocol::{self, DEFAULT_SERVICE_URL, Decision, TARGET_MACOS_ARM64};
use super::{UpdateState, WorkerCmd, build_channel};
use crate::App;

const INITIAL_DELAY: Duration = Duration::from_secs(10);
const HOURLY_INTERVAL: Duration = Duration::from_secs(3600);
const EXPECTED_SIGNING_TEAM: &str = "5NZ4Q7NXJ4";

static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

pub(super) struct StagedUpdate {
    version: String,
    dir: PathBuf,
    app: PathBuf,
    sha256: String,
}

fn update_feed_channel() -> &'static str {
    match build_channel() {
        "stable" => "stable",
        _ => "dev",
    }
}

#[allow(clippy::disallowed_methods)]
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Starts the background updater when running inside `anomaly.opencode-rune`.
pub(super) fn start(s: &mut App) {
    let Some(app_path) = mac::running_bundle() else { return };
    let Ok(bundle) = mac::bundle_info(&app_path) else { return };
    if !mac::is_updatable_bundle_id(&bundle.id) {
        return;
    }
    let Some(cache_root) = update_cache_dir() else { return };
    let channel = env_nonempty("OPENCODE_RUNE_UPDATE_CHANNEL").unwrap_or_else(|| update_feed_channel().to_owned());
    let service = env_nonempty("OPENCODE_RUNE_RELEASE_URL").unwrap_or_else(|| DEFAULT_SERVICE_URL.to_owned());
    let initial_delay = env_nonempty("OPENCODE_RUNE_UPDATE_INITIAL_DELAY_MS")
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(INITIAL_DELAY, Duration::from_millis);
    let auto_install = env_nonempty("OPENCODE_RUNE_AUTO_INSTALL_ON_READY").as_deref() == Some("1");

    s.updater.supported = true;
    s.updater.version = bundle.version.clone();
    s.updater.channel = channel.clone();

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (state_tx, state_rx) = std::sync::mpsc::channel();
    s.updater.cmd = Some(cmd_tx);
    s.updater.rx = Some(state_rx);
    let waker = s.waker.clone();

    let cfg = WorkerConfig { service, channel, bundle, cache_root, initial_delay, auto_install };
    let _ = std::thread::Builder::new()
        .name("opencode-updater".into())
        .spawn(move || worker_loop(cfg, cmd_rx, state_tx, waker));
}

#[allow(clippy::disallowed_methods)]
fn update_cache_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Caches").join(TEAM_BUNDLE_ID).join("Updates"))
}

pub(super) struct WorkerConfig {
    pub(super) service: String,
    pub(super) channel: String,
    pub(super) bundle: BundleInfo,
    pub(super) cache_root: PathBuf,
    pub(super) initial_delay: Duration,
    pub(super) auto_install: bool,
}

pub(super) struct Engine {
    pub(super) cfg: WorkerConfig,
    pub(super) rollout_bucket: u8,
    pub(super) etag: Option<String>,
    pub(super) staged: Option<StagedUpdate>,
    pub(super) check_team: Option<String>,
    pub(super) check_gatekeeper: bool,
}

impl Engine {
    fn new(cfg: WorkerConfig) -> Self {
        sweep_old_stages(&cfg.cache_root);
        let rollout_bucket = load_or_create_rollout_bucket(&cfg.cache_root);
        let local_loopback_test = cfg.service.starts_with("http://127.0.0.1:")
            && cfg.bundle.team.is_none()
            && env_nonempty("OPENCODE_RUNE_SKIP_GATEKEEPER").as_deref() == Some("1");
        let (check_team, check_gatekeeper) = if local_loopback_test {
            (None, false)
        } else {
            let team = cfg.bundle.team.clone().or_else(|| Some(EXPECTED_SIGNING_TEAM.to_owned()));
            (team, true)
        };
        Self { cfg, rollout_bucket, etag: None, staged: None, check_team, check_gatekeeper }
    }

    pub(super) fn check_once(&mut self, emit: &mut dyn FnMut(UpdateState)) -> UpdateState {
        emit(UpdateState::Checking);
        let url = protocol::manifest_url(&self.cfg.service, &self.cfg.channel);
        let ua = format!(
            "opencode-rune/{} ({TARGET_MACOS_ARM64}; channel={})",
            self.cfg.bundle.version, self.cfg.channel
        );
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(180)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut req = agent.get(&url).header("User-Agent", &ua);
        if let Some(etag) = &self.etag {
            req = req.header("If-None-Match", etag);
        }
        let mut resp = match req.call() {
            Ok(r) => r,
            Err(e) => return UpdateState::Failed(format!("Could not reach update server: {e}")),
        };
        let status = resp.status().as_u16();
        if status == 304 {
            return match &self.staged {
                Some(st) => UpdateState::Ready { version: st.version.clone() },
                None => UpdateState::UpToDate,
            };
        }
        if status == 404 {
            return UpdateState::Idle;
        }
        if !(200..300).contains(&status) {
            return UpdateState::Failed(format!("Update server returned HTTP {status}"));
        }
        let next_etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let text = match resp.body_mut().read_to_string() {
            Ok(t) => t,
            Err(e) => return UpdateState::Failed(format!("Could not read manifest: {e}")),
        };
        let manifest = match protocol::parse_manifest(&text, &url, TARGET_MACOS_ARM64) {
            Ok(m) => m,
            Err(e) => return UpdateState::Failed(e),
        };
        self.etag = next_etag;
        let installed_sha = read_installed_sha(&self.cfg.cache_root, &self.cfg.bundle.version);
        let decision = match protocol::decide(
            &manifest,
            &self.cfg.channel,
            &self.cfg.bundle.version,
            installed_sha.as_deref(),
            self.rollout_bucket,
        ) {
            Ok(d) => d,
            Err(e) => return UpdateState::Failed(e),
        };
        let Decision::Install { version, target } = decision else {
            return UpdateState::UpToDate;
        };
        if let Some(st) = &self.staged
            && st.sha256 == target.sha256
            && st.app.exists()
        {
            return UpdateState::Ready { version: st.version.clone() };
        }
        if let Err(e) = mac::check_replaceable(&self.cfg.bundle.path) {
            return UpdateState::Failed(e);
        }
        emit(UpdateState::Downloading { version: version.clone() });
        match self.download_and_stage(&agent, &ua, &version, &target) {
            Ok(staged) => {
                if let Some(prev) = self.staged.replace(staged) {
                    let _ = std::fs::remove_dir_all(prev.dir);
                }
                UpdateState::Ready { version }
            }
            Err(e) => UpdateState::Failed(e),
        }
    }

    fn download_and_stage(
        &self,
        agent: &ureq::Agent,
        ua: &str,
        version: &str,
        target: &protocol::Target,
    ) -> Result<StagedUpdate, String> {
        std::fs::create_dir_all(&self.cfg.cache_root).map_err(|e| format!("could not create update dir: {e}"))?;
        let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = self.cfg.cache_root.join(format!("stage-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not create stage dir: {e}"))?;
        let result = (|| {
            let zip_path = dir.join("update.zip");
            let mut resp = agent
                .get(&target.url)
                .header("User-Agent", ua)
                .call()
                .map_err(|e| format!("download failed: {e}"))?;
            let code = resp.status().as_u16();
            if !(200..300).contains(&code) {
                return Err(format!("download returned HTTP {code}"));
            }
            let mut file = std::fs::File::create(&zip_path).map_err(|e| format!("could not write archive: {e}"))?;
            let mut reader = resp.body_mut().as_reader();
            let mut hasher = Sha256::new();
            let mut buf = [0u8; 64 * 1024];
            let mut written: u64 = 0;
            loop {
                let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
                if n == 0 {
                    break;
                }
                written += n as u64;
                if written > target.size {
                    return Err("downloaded archive exceeded manifest size".into());
                }
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n]).map_err(|e| format!("disk write failed: {e}"))?;
            }
            file.sync_all().map_err(|e| format!("sync failed: {e}"))?;
            if written != target.size {
                return Err(format!("downloaded {written} bytes, expected {}", target.size));
            }
            let digest = hex_lower(&hasher.finalize());
            if digest != target.sha256 {
                return Err("update archive SHA-256 mismatch".into());
            }
            let unpacked = dir.join("unpacked");
            std::fs::create_dir_all(&unpacked).map_err(|e| format!("could not create unpack dir: {e}"))?;
            let app = mac::unpack_zip(&zip_path, &unpacked)?;
            let _ = std::fs::remove_file(&zip_path);
            mac::verify_bundle(&app, &self.cfg.bundle.id, self.check_team.as_deref(), self.check_gatekeeper)?;
            Ok(StagedUpdate { version: version.to_owned(), dir: dir.clone(), app, sha256: digest })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        result
    }

    pub(super) fn apply_staged(&mut self, relaunch: bool) -> Result<(), String> {
        let Some(staged) = self.staged.take() else {
            return Err("No staged update ready to install.".into());
        };
        let check = mac::verify_bundle(
            &staged.app,
            &self.cfg.bundle.id,
            self.check_team.as_deref(),
            self.check_gatekeeper,
        );
        if let Err(e) = check {
            let _ = std::fs::remove_dir_all(&staged.dir);
            return Err(e);
        }
        mac::swap(&staged.app, &self.cfg.bundle.path)?;
        write_installed_sha(&self.cfg.cache_root, &staged.version, &staged.sha256);
        let _ = std::fs::remove_dir_all(&staged.dir);
        if relaunch {
            mac::relaunch_and_exit(&self.cfg.bundle.path)?;
        }
        Ok(())
    }
}

fn worker_loop(
    cfg: WorkerConfig,
    cmd_rx: Receiver<WorkerCmd>,
    state_tx: Sender<UpdateState>,
    waker: Option<Waker>,
) {
    let auto_install = cfg.auto_install;
    let mut next_check = Instant::now() + cfg.initial_delay;
    let mut engine = Engine::new(cfg);
    let mut send = |st: UpdateState| {
        let _ = state_tx.send(st);
        if let Some(w) = &waker {
            w();
        }
    };
    loop {
        let wait = next_check.saturating_duration_since(Instant::now());
        match cmd_rx.recv_timeout(wait) {
            Ok(WorkerCmd::Check) | Err(RecvTimeoutError::Timeout) => {
                let final_state = engine.check_once(&mut send);
                let is_ready = matches!(final_state, UpdateState::Ready { .. });
                send(final_state);
                if is_ready && auto_install {
                    if let Err(e) = engine.apply_staged(true) {
                        send(UpdateState::Failed(e));
                    }
                }
                next_check = Instant::now() + HOURLY_INTERVAL;
            }
            Ok(WorkerCmd::Install) => {
                if let Err(e) = engine.apply_staged(true) {
                    send(UpdateState::Failed(e));
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

pub(super) fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn sweep_old_stages(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("stage-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[allow(clippy::disallowed_methods)]
fn load_or_create_rollout_bucket(root: &Path) -> u8 {
    let seed_file = root.join("rollout-bucket");
    if let Ok(text) = std::fs::read_to_string(&seed_file)
        && let Ok(bucket) = text.trim().parse::<u8>()
        && bucket < 100
    {
        return bucket;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    let bucket = digest[0] % 100;
    let _ = std::fs::create_dir_all(root);
    let _ = std::fs::write(&seed_file, format!("{bucket}\n"));
    bucket
}

fn read_installed_sha(root: &Path, running_version: &str) -> Option<String> {
    let raw = std::fs::read_to_string(root.join("installed-release")).ok()?;
    let (ver, sha) = raw.trim().split_once(' ')?;
    (ver == running_version && sha.len() == 64).then(|| sha.to_owned())
}

fn write_installed_sha(root: &Path, version: &str, sha256: &str) {
    let _ = std::fs::create_dir_all(root);
    let _ = std::fs::write(root.join("installed-release"), format!("{version} {sha256}\n"));
}
