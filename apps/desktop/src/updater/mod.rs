//! Keeps team builds (`anomaly.opencode-rune`) current from `https://anoma.ly/release`.
//!
//! Checks 10 seconds after launch and hourly afterwards, downloads and verifies updates in a
//! private folder under `~/Library/Caches/anomaly.opencode-rune/Updates`, and swaps the bundle
//! atomically when the user presses Restart to update. Kit's local builds
//! (`com.kitlangton.opencode-rune-desktop`) and bare `cargo` binaries never run this updater.

pub mod mac;
pub mod protocol;

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use rune::{Color, El, Play, TextStyle, Tx, Waker, col, el, row, text};
use sha2::{Digest as _, Sha256};

use crate::App;
use crate::commands::Cmd;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::view::{INSTANT, UI, UI_MED};
use mac::{BundleInfo, TEAM_BUNDLE_ID};
use protocol::{DEFAULT_SERVICE_URL, Decision, TARGET_MACOS_ARM64};

const INITIAL_DELAY: Duration = Duration::from_secs(10);
const HOURLY_INTERVAL: Duration = Duration::from_secs(3600);
const EXPECTED_SIGNING_TEAM: &str = "5NZ4Q7NXJ4";

static STAGE_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
    },
    Ready {
        version: String,
    },
    UpToDate,
    Failed(String),
}

enum WorkerCmd {
    Check,
    Install,
}

struct StagedUpdate {
    version: String,
    dir: PathBuf,
    app: PathBuf,
    sha256: String,
}

#[derive(Default)]
pub struct Updater {
    supported: bool,
    story_visible: bool,
    pub version: String,
    pub channel: String,
    pub state: UpdateState,
    cmd: Option<Sender<WorkerCmd>>,
    rx: Option<Receiver<UpdateState>>,
}

impl Updater {
    pub fn supported(&self) -> bool {
        self.supported
    }

    pub fn visible(&self) -> bool {
        self.supported || self.story_visible
    }

    pub fn check(&mut self, tx: &mut Tx) {
        if let Some(cmd) = &self.cmd {
            tx.instant();
            self.state = UpdateState::Checking;
            let _ = cmd.send(WorkerCmd::Check);
        } else if self.story_visible {
            tx.instant();
            self.state = UpdateState::Checking;
        }
    }

    pub fn install(&mut self, tx: &mut Tx) {
        if let Some(cmd) = &self.cmd {
            tx.instant();
            let _ = cmd.send(WorkerCmd::Install);
        } else if self.story_visible {
            tx.instant();
            self.state = UpdateState::UpToDate;
        }
    }
}

pub fn build_channel() -> &'static str {
    match option_env!("OPENCODE_RUNE_BUILD_CHANNEL") {
        Some(ch) if !ch.is_empty() => ch,
        _ => "local",
    }
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
pub fn start_if_packaged(s: &mut App) {
    if crate::headless() || cfg!(test) || s.updater.cmd.is_some() {
        return;
    }
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

/// Drains updater status updates on the UI thread; zero allocations when idle.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let Some(rx) = &s.updater.rx else { return false };
    let mut next = None;
    loop {
        match rx.try_recv() {
            Ok(st) => next = Some(st),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                s.updater.rx = None;
                s.updater.cmd = None;
                break;
            }
        }
    }
    let Some(st) = next else { return false };
    if s.updater.state == st {
        return false;
    }
    tx.instant();
    s.updater.state = st;
    true
}

#[allow(clippy::disallowed_methods)]
fn update_cache_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Caches").join(TEAM_BUNDLE_ID).join("Updates"))
}

struct WorkerConfig {
    service: String,
    channel: String,
    bundle: BundleInfo,
    cache_root: PathBuf,
    initial_delay: Duration,
    auto_install: bool,
}

struct Engine {
    cfg: WorkerConfig,
    rollout_bucket: u8,
    etag: Option<String>,
    staged: Option<StagedUpdate>,
    check_team: Option<String>,
    check_gatekeeper: bool,
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

    fn check_once(&mut self, emit: &mut dyn FnMut(UpdateState)) -> UpdateState {
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

    fn apply_staged(&mut self, relaunch: bool) -> Result<(), String> {
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

fn hex_lower(bytes: &[u8]) -> String {
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

// ---------------------------------------------------------------------------------------------
// Views, Command, and Storybook story

const SECTION: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);

/// Compact pill pinned at the bottom of the sidebar while an update is downloaded and verified.
pub fn sidebar_pill(s: &App, p: &Pal) -> Option<El> {
    let UpdateState::Ready { version } = &s.updater.state else { return None };
    Some(
        row()
            .key("updater-ready-pill")
            .role("button")
            .label("Restart to update")
            .help(format!("Restart to apply {version}"))
            .mx(10.0)
            .mb(10.0)
            .h(32.0)
            .px(10.0)
            .gap(8.0)
            .center_y()
            .radius(6.0)
            .fill(p.layer2)
            .border(0.5, p.ring)
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.updater.install(tx))
            .child(el().size(6.0, 6.0).radius(3.0).fill(p.bg_accent))
            .child(text("Update ready").font(UI).color(p.text).grow(1.0).shrink().truncate_fade())
            .child(text("Restart").font(UI_MED).color(p.muted)),
    )
}

/// Settings section shown on packaged team builds (`anomaly.opencode-rune`) and in its story.
pub fn settings_section(s: &App, p: &Pal) -> Option<El> {
    if !s.updater.visible() {
        return None;
    }
    let ver = if s.updater.version.is_empty() { env!("CARGO_PKG_VERSION") } else { &s.updater.version };
    let ch = if s.updater.channel.is_empty() { build_channel() } else { &s.updater.channel };
    let (about, btn_label, is_ready, busy) = match &s.updater.state {
        UpdateState::Idle => (format!("Version {ver} ({ch}). Checks automatically every hour."), "Check for updates", false, false),
        UpdateState::Checking => (format!("Version {ver} ({ch}). Checking for updates…"), "Checking…", false, true),
        UpdateState::Downloading { version } => (format!("Downloading {version}…"), "Downloading…", false, true),
        UpdateState::Ready { version } => (format!("{version} is verified and ready to install."), "Restart to update", true, false),
        UpdateState::UpToDate => (format!("Version {ver} ({ch}) is up to date."), "Check for updates", false, false),
        UpdateState::Failed(err) => (format!("Version {ver} ({ch}). {err}"), "Try again", false, false),
    };
    let mut btn = row()
        .key("updater-action")
        .role("button")
        .label(btn_label)
        .h(28.0)
        .px(12.0)
        .center()
        .radius(6.0)
        .fill(if is_ready { p.bg_accent } else { p.bg_base })
        .border(0.5, if is_ready { Color::TRANSPARENT } else { p.ring })
        .hover(if busy { p.bg_base } else if is_ready { p.bg_accent } else { p.hover })
        .hover_motion(INSTANT, INSTANT)
        .child(text(btn_label).font(UI_MED).color(if is_ready { p.switch_thumb } else if busy { p.faint } else { p.text }));
    if !busy {
        btn = btn.press(p.pressed).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if is_ready {
                s.updater.install(tx);
            } else {
                s.updater.check(tx);
            }
        });
    }
    let card = col()
        .key("updater-card")
        .w_full()
        .px(20.0)
        .py(20.0)
        .radius(8.0)
        .fill(p.layer2)
        .child(
            row()
                .w_full()
                .gap(16.0)
                .center_y()
                .child(
                    col()
                        .grow(1.0)
                        .shrink()
                        .gap(8.0)
                        .child(text("Automatic updates").font(UI_MED).color(p.text))
                        .child(text(about).key("updater-about").font(BODY).color(p.muted).wrap()),
                )
                .child(btn),
        );
    Some(col().key("updater-section").w_full().gap(16.0).child(text("Updates").font(SECTION).color(p.text)).child(card))
}

rune_register::register!(crate::commands::COMMANDS: Cmd = [
    Cmd::new("updater.check", "Check for updates", |s, tx| {
        s.open_settings();
        s.updater.check(tx);
    })
    .describe("Check the Anomaly release feed for a newer build")
    .when(|s| s.updater.supported()),
]);

const STORY_VARIANTS: &[(&str, usize)] = &[("Ready to restart", 0), ("Up to date", 1), ("Checking", 2)];

const STORY_KEYS: &[Trigger] = &[
    replay(story_open),
    Trigger { key: 'c', label: "Check for updates", run: |s, tx| s.updater.check(tx) },
];

fn story_setup(s: &mut App) {
    s.updater.story_visible = true;
    s.updater.version = "0.0.0-dev-41".into();
    s.updater.channel = "dev".into();
    s.updater.state = match s.variant {
        1 => UpdateState::UpToDate,
        2 => UpdateState::Checking,
        _ => UpdateState::Ready { version: "0.0.0-dev-42".into() },
    };
}

fn story_open(s: &mut App, tx: &mut Play<App>) {
    story_setup(s);
    tx.instant();
}

fn story_view(s: &App, p: &Pal) -> El {
    col()
        .key("updater-story")
        .w(680.0)
        .padding(28.0)
        .gap(20.0)
        .radius(10.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .children(settings_section(s, p))
        .children(sidebar_pill(s, p).map(|pill| col().w(260.0).child(pill)))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Settings/Updates",
        scene: |s| { story_setup(s); stage(s, story_view); },
        knobs: &[],
        keys: STORY_KEYS,
        variants: STORY_VARIANTS,
    },
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("settings-updates", |s| {
        crate::open_scene(s);
        s.finish_stream();
        s.open_settings();
        story_setup(s);
    }),
]);

#[cfg(test)]
mod tests;
