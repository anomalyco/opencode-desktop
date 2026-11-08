//! Keeps OpenCode (Rune) current from Anomaly's release feed.
//!
//! The update engine is a host service ([`service::UpdaterService`]: a state machine, download
//! progress, and check and install actions) over a platform updater:
//!
//! - **macOS** (`mac.rs`): team builds (`anomaly.opencode-rune`) download and verify a signed
//!   bundle (code signature, signing team, Gatekeeper) into a private folder under
//!   `~/Library/Caches/anomaly.opencode-rune/Updates`, and swap it in atomically on Restart to
//!   update. Local dev bundles and bare `cargo` binaries never update.
//! - **Windows** (`windows.rs`): a per-user install verifies the download (size, SHA-256,
//!   Authenticode), stages it beside the install folder, and swaps the folders once the app exits
//!   (`apply.rs`, the app's own executable as the helper), on Restart to update or on quit.
//!
//! Both check ten seconds after launch and every ten minutes. This module draws the service's
//! state: the Updates section in Settings, the sidebar's Update ready pill, and the Check for
//! updates command.

#[cfg(windows)]
pub mod apply;
#[cfg(windows)]
mod authenticode;
mod engine;
mod feed;
#[cfg(target_os = "macos")]
pub mod mac;
mod machine;
mod platform;
pub mod protocol;
pub mod service;
#[cfg(windows)]
mod windows;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rune::{Color, El, Play, TextStyle, Tx, col, el, row, text};

use self::machine::View;
use self::service::UpdaterService;
use crate::App;
use crate::commands::Cmd;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::view::{INSTANT, UI, UI_MED};

/// What the UI shows of the service's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    /// No updater runs in this build (a development build, or no release feed for it yet), so
    /// Settings has no Updates section.
    #[default]
    Hidden,
    Idle,
    Checking,
    Downloading {
        version: String,
    },
    Ready {
        version: String,
    },
    Installing {
        version: String,
    },
    UpToDate,
    Failed(String),
}

impl From<&View> for UpdateState {
    fn from(view: &View) -> Self {
        match view {
            View::Disabled => UpdateState::Hidden,
            View::Idle => UpdateState::Idle,
            View::Checking => UpdateState::Checking,
            View::Downloading(version) => UpdateState::Downloading { version: version.clone() },
            View::UpToDate => UpdateState::UpToDate,
            View::Ready(version) => UpdateState::Ready { version: version.clone() },
            View::Installing(version) => UpdateState::Installing { version: version.clone() },
            View::Error(message) => UpdateState::Failed(message.clone()),
        }
    }
}

/// What carries out a check or an install.
#[derive(Default)]
enum Driver {
    /// Nothing: no updater runs in this build.
    #[default]
    None,
    /// The update service of an installed build, which every window shares.
    Service(Rc<RefCell<UpdaterService>>),
    /// The Settings/Updates story, which plays each step itself.
    Story,
}

#[derive(Default)]
pub struct Updater {
    pub version: String,
    pub channel: String,
    pub state: UpdateState,
    driver: Driver,
    /// The service's revision this window last showed.
    seen: u64,
}

thread_local! {
    /// The one update service of the app (its windows share the app's thread), started by the
    /// first window, and how many windows use it.
    static SHARED: RefCell<Option<Rc<RefCell<UpdaterService>>>> = const { RefCell::new(None) };
    static WINDOWS: Cell<usize> = const { Cell::new(0) };
}

impl Updater {
    pub fn supported(&self) -> bool {
        matches!(self.driver, Driver::Service(_))
    }

    /// The update service, while one runs: the interface a UI drives (state, progress, check,
    /// install).
    pub fn service(&self) -> Option<Rc<RefCell<UpdaterService>>> {
        match &self.driver {
            Driver::Service(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn check(&mut self, tx: &mut Tx) {
        match &mut self.driver {
            Driver::Service(service) => {
                tx.instant();
                let mut service = service.borrow_mut();
                service.check();
                self.state = UpdateState::from(&service.snapshot().state);
            }
            Driver::Story => {
                tx.instant();
                self.state = UpdateState::Checking;
            }
            Driver::None => {}
        }
    }

    pub fn install(&mut self, tx: &mut Tx) {
        match &mut self.driver {
            Driver::Service(service) => {
                tx.instant();
                let mut service = service.borrow_mut();
                service.install();
                self.state = UpdateState::from(&service.snapshot().state);
            }
            Driver::Story => {
                tx.instant();
                self.state = UpdateState::UpToDate;
            }
            Driver::None => {}
        }
    }

    /// This window closes. When it's the last, the app quits, and a ready update installs on the
    /// way out (Windows).
    pub fn closing(&mut self) {
        let Driver::Service(service) = std::mem::take(&mut self.driver) else { return };
        let left = WINDOWS.get().saturating_sub(1);
        WINDOWS.set(left);
        if left == 0 {
            service.borrow_mut().quitting();
            SHARED.with(|s| s.borrow_mut().take());
        }
    }
}

pub fn build_channel() -> &'static str {
    match option_env!("OPENCODE_RUNE_BUILD_CHANNEL") {
        Some(ch) if !ch.is_empty() => ch,
        _ => "local",
    }
}

/// This build's version: `OPENCODE_RUNE_VERSION` when the build set it (a release build sets the
/// crate's version to it too), else the crate's.
pub fn build_version() -> &'static str {
    match option_env!("OPENCODE_RUNE_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
}

/// Starts the update service when this build has an updater (`platform::make`), or joins this
/// window to the one the first window started.
pub fn start_if_packaged(s: &mut App) {
    if crate::headless() || cfg!(test) || s.updater.supported() {
        return;
    }
    let service = SHARED.with(|shared| {
        if let Some(service) = shared.borrow().as_ref() {
            return Some(service.clone());
        }
        let (exe, updates) = (std::env::current_exe().ok()?, crate::dirs::updates()?);
        let platform = platform::make(&exe, build_version(), build_channel(), updates.clone())?;
        let service = Rc::new(RefCell::new(UpdaterService::start(platform, feed::feed_channel(build_channel()), updates)));
        *shared.borrow_mut() = Some(service.clone());
        Some(service)
    });
    let Some(service) = service else { return };
    WINDOWS.set(WINDOWS.get() + 1);
    {
        let service = service.borrow();
        if let Some(waker) = s.waker.clone() {
            service.add_waker(waker);
        }
        s.updater.version = service.snapshot().version.clone();
        s.updater.channel = service.snapshot().channel.clone();
        s.updater.state = UpdateState::from(&service.snapshot().state);
        s.updater.seen = service.revision();
    }
    s.updater.driver = Driver::Service(service);
}

/// Feeds the service's results to this window on the app's thread; zero allocations when idle.
/// An install that handed off quits the app, which saves every window first (`App::on_quit`),
/// so the update opens each of them again.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let Driver::Service(service) = &s.updater.driver else { return false };
    let (quit, state, revision) = {
        let mut service = service.borrow_mut();
        service.poll();
        if service.revision() == s.updater.seen && !service.quit_pending() {
            return false;
        }
        (service.take_quit(), UpdateState::from(&service.snapshot().state), service.revision())
    };
    s.updater.seen = revision;
    if quit {
        tx.quit();
    }
    if s.updater.state == state {
        return false;
    }
    tx.instant();
    s.updater.state = state;
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
            .label(opencode_i18n::t!("rune.updater.restart"))
            .help(opencode_i18n::t!("rune.updater.restartApply", version = version))
            .mx(10.0)
            .mb(10.0)
            .h(32.0)
            .px(10.0)
            .gap(8.0)
            .center_y()
            .radius(6.0)
            .fill(p.layer2)
            .box_shadow(Pal::ring_out(p.ring))
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.updater.install(tx))
            .child(el().size(6.0, 6.0).radius(3.0).fill(p.bg_accent))
            .child(text(opencode_i18n::t!("plugin.updater.web.ready.title")).font(UI).color(p.text).grow(1.0).shrink().truncate_fade())
            .child(text(opencode_i18n::t!("plugin.updater.dialog.restart")).font(UI_MED).color(p.muted)),
    )
}

/// Settings section shown while the update service runs, and in its story.
pub fn settings_section(s: &App, p: &Pal) -> Option<El> {
    let ver = if s.updater.version.is_empty() { build_version() } else { &s.updater.version };
    let ch = if s.updater.channel.is_empty() { build_channel() } else { &s.updater.channel };
    let (about, action) = match &s.updater.state {
        UpdateState::Hidden => return None,
        UpdateState::Idle => (opencode_i18n::t!("rune.updater.idle", version = ver, channel = ch), Some((opencode_i18n::t!("plugin.updater.check.title"), false, false))),
        UpdateState::Checking => (opencode_i18n::t!("rune.updater.checking", version = ver, channel = ch), Some((opencode_i18n::t!("plugin.updater.action.checking"), false, true))),
        UpdateState::Downloading { version } => (opencode_i18n::t!("rune.updater.downloading", version = version), Some((opencode_i18n::t!("plugin.updater.action.downloading"), false, true))),
        UpdateState::Ready { version } => (opencode_i18n::t!("rune.updater.ready", version = version), Some((opencode_i18n::t!("rune.updater.restart"), true, false))),
        UpdateState::UpToDate => (opencode_i18n::t!("rune.updater.upToDate", version = ver, channel = ch), Some((opencode_i18n::t!("plugin.updater.check.title"), false, false))),
        UpdateState::Failed(err) => (opencode_i18n::t!("rune.updater.failed", version = ver, channel = ch, error = err), Some((opencode_i18n::t!("common.retry"), false, false))),
        UpdateState::Installing { version } => (opencode_i18n::t!("rune.updater.installing", version = version), Some((opencode_i18n::t!("plugin.updater.action.installing"), false, true))),
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
            .box_shadow(Pal::ring_out(if is_ready { Color::TRANSPARENT } else { p.ring }))
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
                        .child(text(opencode_i18n::t!("rune.updater.automatic")).font(UI_MED).color(p.text))
                        .child(text(about).key("updater-about").font(BODY).color(p.muted).wrap()),
                )
                .children(btn),
        );
    Some(col().key("updater-section").w_full().gap(16.0).child(text(opencode_i18n::t!("plugin.updater.section.title")).font(SECTION).color(p.text)).child(card))
}

rune_register::register!(crate::commands::COMMANDS: Cmd = [
    // Desktop's title, as its catalog lists it, and kept out of the palette as there
    // (`gui-plugins/src/updater/renderer.tsx:120`, `hidden: true`).
    Cmd::new("updater.check", opencode_i18n::key!("plugin.updater.menu.check"), |s, tx| {
        s.open_settings();
        s.updater.check(tx);
    })
    .describe(opencode_i18n::key!("rune.command.updater.check.description"))
    .when(|s| s.updater.supported())
    .hidden(),
]);

const STORY_VARIANTS: &[(&str, usize)] = &[("Ready to restart", 0), ("Up to date", 1), ("Checking", 2)];

const STORY_KEYS: &[Trigger] = &[
    replay(story_open),
    Trigger { key: 'c', label: "Check for updates", run: |s, tx| s.updater.check(tx) },
];

fn story_setup(s: &mut App) {
    s.updater.driver = Driver::Story;
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
        .box_shadow(Pal::ring_out(p.ring))
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
