//! Desktop's built-in Debug plugin (`packages/gui-plugins/src/debug`) as a native plugin: the
//! debug bar across the window's foot (Toggle debug bar), Capture heap snapshots, and in debug
//! builds memory diagnostics. The bar's motion tools drive the app's own: SPEED and SLOW its
//! animation clock (Knobs › Global), KNOBS the Knobs panel, COMPONENTS the components playground.
//!
//! A built-in: this rlib declares it, `plugins/debug` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod bar;
mod heap;
mod memlog;
mod turn;

use std::hash::{Hash, Hasher};

use opencode_plugin::prelude::*;
use opencode_plugin::system;

/// Whether the bar, and the Knobs panel with it, are open; kept across launches (`bar`).
const BAR: Store<Bar> = Store::new("bar");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bar {
    visible: bool,
    motion: bool,
}

impl StoreValue for Bar {
    fn encode(&self) -> String {
        serde_json::json!({ "visible": self.visible, "motion": self.motion }).to_string()
    }
    fn decode(text: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Bar { visible: v["visible"].as_bool()?, motion: v["motion"].as_bool()? })
    }
}

/// How the bar shows (`debugBarView`): running diagnostics pin it open, without its close
/// button, so they're never silently on.
pub struct View {
    pub open: bool,
    pub closable: bool,
    pub togglable: bool,
}

pub struct Debug {
    heap: heap::Heap,
    memlog: memlog::MemoryLog,
    /// Debug builds (local and Beta) run memory diagnostics.
    debug_build: bool,
    /// Local and Beta builds have the motion tools.
    tools: bool,
    /// The developer tools' state as last drawn, to redraw when it changes.
    dev: [Option<String>; 3],
    started: bool,
}

impl WindowPlugin for Debug {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        let channel = system::channel();
        let debug_build = matches!(channel.as_str(), "local" | "beta");
        cx.add_command(CommandSpec::new("toggle", "Toggle debug bar").keywords("dev bar developer tools diagnostics"), |s, cx| {
            // Running diagnostics keep the bar open.
            if s.view().togglable {
                let bar = BAR.get().unwrap_or_default();
                if bar.visible { s.hide(cx) } else { s.show(cx) }
            }
        });
        cx.add_command(CommandSpec::new("heap", "Capture heap snapshots").description("Saves the memory of the app and its server to a folder"), |s, cx| {
            s.heap.run(cx);
            s.schedule(cx);
        });
        cx.add_slot(Contribution::slot(SlotName::WindowBottom, "bar"), bar::render);
        Ok(Debug { heap: heap::Heap::default(), memlog: memlog::MemoryLog::new(), debug_build, tools: debug_build, dev: Default::default(), started: false })
    }

    /// Starts diagnostics and the stored Knobs panel once, writes the sample and ends the heap
    /// capture that came due, and redraws when the developer tools changed.
    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let now = cx.now_ms();
        let mut changed = false;
        if !self.started {
            self.started = true;
            if self.debug_build {
                self.memlog.sync(now);
            }
            let bar = BAR.get().unwrap_or_default();
            if self.tools && self.view().open && bar.motion && system::dev("knobs").as_deref() != Some("on") {
                cx.dev("knobs");
            }
            changed = true;
        }
        self.memlog.poll(now);
        if let Some(attempt) = self.heap.running.as_ref().filter(|c| c.deadline <= now).map(|c| c.attempt) {
            self.heap.finish(cx, attempt, serde_json::json!({ "error": "The operation was aborted due to timeout" }));
            changed = true;
        }
        let dev = ["speed", "slow", "knobs"].map(system::dev);
        if dev != self.dev {
            self.dev = dev;
            changed = true;
        }
        self.schedule(cx);
        changed
    }
}

impl Debug {
    pub fn view(&self) -> View {
        let pinned = self.memlog.on();
        let visible = pinned || BAR.get().unwrap_or_default().visible;
        View { open: visible, closable: visible && !pinned, togglable: !pinned }
    }

    fn show(&mut self, cx: &mut ActionCx) {
        let bar = BAR.get().unwrap_or_default();
        BAR.set(&Bar { visible: true, ..bar });
        // The Knobs panel shows while the bar does.
        if self.tools && bar.motion && system::dev("knobs").as_deref() != Some("on") {
            cx.dev("knobs");
        }
    }

    pub(crate) fn hide(&mut self, cx: &mut ActionCx) {
        let bar = BAR.get().unwrap_or_default();
        BAR.set(&Bar { visible: false, ..bar });
        if self.tools && bar.motion && system::dev("knobs").as_deref() == Some("on") {
            cx.dev("knobs");
        }
    }

    pub(crate) fn toggle_knobs(&mut self, cx: &mut ActionCx) {
        let open = system::dev("knobs").as_deref() == Some("on");
        let bar = BAR.get().unwrap_or_default();
        BAR.set(&Bar { motion: !open, ..bar });
        cx.dev("knobs");
    }

    /// Wakes for the next memory sample and the heap capture's deadline.
    pub(crate) fn schedule(&self, cx: &mut ActionCx) {
        let now = cx.now_ms();
        let next = [self.memlog.due(), self.heap.running.as_ref().map(|c| c.deadline)].into_iter().flatten().min();
        if let Some(at) = next {
            cx.wake_after((at - now).max(0) as u64);
        }
    }
}

pub(crate) fn hash(value: &impl Hash) -> u64 {
    let mut h = opencode_plugin::FxHasher::default();
    value.hash(&mut h);
    h.finish()
}

/// `toISOString()` of `ms` since the epoch: `2026-11-08T12:34:56.789Z`.
pub(crate) fn iso(ms: i64) -> String {
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000)
}

/// [`iso`] without `:` and `.`, for file names (`.replace(/[:.]/g, "")`).
pub(crate) fn stamp(ms: i64) -> String {
    iso(ms).replace([':', '.'], "")
}

opencode_plugin::declare_plugin!(window = Debug);
