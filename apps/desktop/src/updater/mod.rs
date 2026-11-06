//! Keeps team builds (`anomaly.opencode-rune`) current from `https://anoma.ly/release`.
//!
//! On macOS, checks 10 seconds after launch and hourly afterwards, downloads and verifies updates
//! in a private folder under `~/Library/Caches/anomaly.opencode-rune/Updates`, and swaps the bundle
//! atomically when the user presses Restart to update. Local dev builds
//! (`anomaly.opencode-rune.dev`) and bare `cargo` binaries never run this updater. Other platforms
//! have no updater yet.

#[cfg(target_os = "macos")]
pub mod mac;
// The worker is macOS-only; elsewhere only the protocol's tests build it.
#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod protocol;
#[cfg(target_os = "macos")]
mod worker;

use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use rune::{Color, El, Play, TextStyle, Tx, col, el, row, text};

use crate::App;
use crate::commands::Cmd;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::view::{INSTANT, UI, UI_MED};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    #[cfg_attr(target_os = "macos", default)]
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
    /// No updater for this platform yet; Settings says so instead of offering a check.
    #[cfg_attr(not(target_os = "macos"), default)]
    Unsupported,
}

enum WorkerCmd {
    Check,
    Install,
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
        self.supported || self.story_visible || self.state == UpdateState::Unsupported
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

/// Starts the background updater when running inside `anomaly.opencode-rune`.
#[cfg(target_os = "macos")]
pub fn start_if_packaged(s: &mut App) {
    if !crate::headless() && !cfg!(test) && s.updater.cmd.is_none() {
        worker::start(s);
    }
}

/// Other platforms have no updater yet: their state stays `Unsupported`, and Settings says so.
#[cfg(not(target_os = "macos"))]
pub fn start_if_packaged(_: &mut App) {}

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

/// Settings section shown on packaged team builds (`anomaly.opencode-rune`), in its story, and on
/// platforms without an updater yet.
pub fn settings_section(s: &App, p: &Pal) -> Option<El> {
    if !s.updater.visible() {
        return None;
    }
    let ver = if s.updater.version.is_empty() { env!("CARGO_PKG_VERSION") } else { &s.updater.version };
    let ch = if s.updater.channel.is_empty() { build_channel() } else { &s.updater.channel };
    let (about, action) = match &s.updater.state {
        UpdateState::Idle => (format!("Version {ver} ({ch}). Checks automatically every hour."), Some(("Check for updates", false, false))),
        UpdateState::Checking => (format!("Version {ver} ({ch}). Checking for updates…"), Some(("Checking…", false, true))),
        UpdateState::Downloading { version } => (format!("Downloading {version}…"), Some(("Downloading…", false, true))),
        UpdateState::Ready { version } => (format!("{version} is verified and ready to install."), Some(("Restart to update", true, false))),
        UpdateState::UpToDate => (format!("Version {ver} ({ch}) is up to date."), Some(("Check for updates", false, false))),
        UpdateState::Failed(err) => (format!("Version {ver} ({ch}). {err}"), Some(("Try again", false, false))),
        UpdateState::Unsupported => (format!("Version {ver} ({ch}). Automatic updates aren't available on this platform yet."), None),
    };
    let btn = action.map(|(btn_label, is_ready, busy)| {
        let btn = row()
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
        if busy {
            return btn;
        }
        btn.press(p.pressed).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if is_ready {
                s.updater.install(tx);
            } else {
                s.updater.check(tx);
            }
        })
    });
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
                .children(btn),
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
