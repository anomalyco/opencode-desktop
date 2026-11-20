//! Subagents as Desktop shows them: the running-work capsule above the composer
//! (`app/src/session/composer/activity-chips.tsx`), background shell tracking and output
//! followers (`shells`), and a subagent's own page status bar (`reply`).
//!
//! A subagent runs while its child session, or any session below it, runs, or while its tree
//! still runs a background job. A background call's tool part finishes at once, so the call's
//! own status only counts until it names its child.

#![forbid(unsafe_code)]

pub mod activity;
pub mod jobs;
pub mod reply;
pub mod shells;

use std::collections::HashSet;

use opencode_kit::model::{self, Message, Part, SessionInfo, Transcript};
use rune::{El, KeyEvent, Tx, col, row, text};
use rune_knobs::{Knob, Knobs};
use session_view::odometer::odometer;
use session_view::palette::{Elevate as _, Pal};
use session_view::style::{INSTANT, UI, UI_MED, activity_ring_sized};

pub use session_view::knobs::CAPSULE_RADIUS;
pub use session_view::subagent_card::{elapsed, run_time};
pub use shells::follow as shell_follow;
pub use shells::tail as shell_tail;
pub use shells::view as shells_view;
pub use shells::{JobListReply, Shell, Shells, StopTarget, SubagentJob, SubagentReply, SubagentRequest, apply_reply, execute_request};

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0).about("How solid the subagent capsule fill is");
/// Its hairline: 1 is Desktop's raised ring (the composer's), 0 the faint muted border.
pub const CAPSULE_RING: Knob = Knob::new("Capsule ring", 0.0, 1.0, 1.0).about("Strength of the subagent capsule hairline");

/// A composer pill's height (`composer-lane.css:7`).
pub const CHIP_H: f32 = session_view::jump::CHIP_H;

/// One running subagent of a session, for the capsule's list.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub session: String,
    pub agent: String,
    pub label: String,
    pub started: i64,
}

/// Object-safe host boundary for the subagents activity capsule, background shells, shell output
/// followers, and subagent reply bar.
pub trait SubagentsHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback {
    /// Background shells state (tracked read).
    fn shells(&self) -> &Shells;
    /// Background shells state (tracked write — callers only invoke when mutating).
    fn shells_mut(&mut self) -> &mut Shells;

    /// Metadata for session `id`, if loaded.
    fn session_info(&self, id: &str) -> Option<&SessionInfo>;
    /// All loaded session metadata (used by [`child_of`] to resolve unnamed subagent calls).
    fn sessions(&self) -> &[SessionInfo];
    /// Transcript for session `id`, if loaded (untracked read preferred to avoid unrelated rebuilds).
    fn transcript(&self, id: &str) -> Option<&Transcript>;
    /// Monotonic version of the host's transcripts map, used to memoize followable shell calls
    /// across polls. Returns `None` when the host does not track transcript versions.
    fn transcripts_version(&self) -> Option<u64> {
        None
    }
    /// Set of currently running session IDs.
    fn running_sessions(&self) -> &HashSet<String>;
    /// When `child`'s current run started (epoch ms), combining `created`, `idle`, `call`, and
    /// any live-observed start timestamp tracked by the host.
    fn run_started(&self, child: &str, call: Option<i64>) -> i64 {
        let info = self.session_info(child);
        let created = info.map_or(0, |i| i.created);
        let idle = info.and_then(|i| i.idle).unwrap_or(0);
        created.max(idle).max(call.unwrap_or(0))
    }

    /// Whether background activity (subagents and background shells) is shown over the composer.
    fn background_activity_enabled(&self) -> bool {
        true
    }
    /// Whether the composer has no text, attachments, or quotes (used for Esc-to-parent on a
    /// subagent's page).
    fn composer_is_empty(&self) -> bool {
        false
    }
    /// Whether the window is at least 640 pt wide (`sm:` breakpoint), showing the `ESC` keycap
    /// on the subagent status bar.
    fn shows_esc_keycap(&self) -> bool {
        true
    }
    /// Whether shell call `call`'s timeline console is currently open (and thus follows its
    /// shell's live output).
    fn is_shell_console_open(&self, call: &session_view::tools::Call<'_>) -> bool {
        let _ = call;
        false
    }

    /// Live look-and-feel knobs (capsule fill/ring/radius, popover motion).
    fn knobs(&self) -> &Knobs;

    /// Prefetches session `id` in the background (on row layout/hover in the activity list).
    fn prefetch_session(&self, id: &str) {
        let _ = id;
    }
    /// Opens subagent `child` from `parent` (inserting placeholder `SessionInfo` + fetching info
    /// when not yet loaded, then navigating to `child`).
    fn open_subagent_session(&mut self, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx);
    /// Navigates to session `id` (used when returning from a subagent to its parent).
    fn navigate_to_session(&mut self, id: &str, tx: &mut Tx);
    /// Called after the activity capsule toggles (e.g. to close sibling composer popovers such
    /// as the todo list).
    fn on_activity_toggled(&mut self, open: bool) {
        let _ = open;
    }
    /// Schedules a background shell/subagent server request (or executes inline when
    /// `sync_if_headless` is true in headless mode).
    fn spawn_shell_request(&mut self, job: SubagentJob);

    /// Identifies the currently active server so per-server job trackers and `Unsupported` flags
    /// stay isolated across server switches.
    fn server_scope(&self) -> &str {
        ""
    }

    /// Dedicated whole-second clock for the open activity panel's elapsed labels. Kept outside
    /// [`Shells`] so 1-second ticks dirty only the open panel, never the timeline (`ours.md` §3.2).
    fn activity_clock_sec(&self) -> i64 {
        self.now_ms().div_euclid(1000)
    }

    /// Updates the host's dedicated activity-panel clock second.
    fn set_activity_clock_sec(&mut self, _second: i64) {}
}

/// `id` and its loaded parents, up to 32 steps.
pub fn ancestors<'a>(host: &'a dyn SubagentsHost, id: &'a str) -> impl Iterator<Item = &'a str> {
    std::iter::successors(Some(id), move |cur| host.session_info(cur)?.parent.as_deref()).take(33)
}

/// Whether `id` is `ancestor` or a session below it.
pub fn descends(host: &dyn SubagentsHost, id: &str, ancestor: &str) -> bool {
    ancestors(host, id).take(32).any(|cur| cur == ancestor)
}

/// Whether `id` or anything below it is running.
pub fn subtree_running(host: &dyn SubagentsHost, id: &str) -> bool {
    host.running_sessions().iter().any(|r| descends(host, r, id))
}

/// The clock running times count to: the host clock's last whole second.
pub fn run_clock(host: &dyn SubagentsHost) -> i64 {
    host.now_ms().div_euclid(1000) * 1000
}

/// The open activity panel's elapsed-time clock, backed by the host's dedicated clock slot so
/// ticking never dirties [`Shells`] or the timeline (`ours.md` §3.2).
pub(crate) fn panel_clock(host: &dyn SubagentsHost) -> i64 {
    host.activity_clock_sec() * 1000
}

/// The sessions running below `id`, and idle descendants whose tree still runs a background job,
/// oldest first, named and dated by their latest call in `id`'s transcript when there is one
/// (`requests/background.ts`'s `subagents`, anomalyco/opencode#64790, #64951).
pub fn running_subagents(host: &dyn SubagentsHost, id: &str) -> Vec<Running> {
    let mut below: Vec<&str> = host.running_sessions().iter().map(String::as_str).collect();
    below.extend(jobs::running(host).flat_map(|j| ancestors(host, j.owner()).take(32)));
    below.sort_unstable();
    below.dedup();
    below.retain(|r| *r != id && descends(host, r, id));
    if below.is_empty() {
        return Vec::new();
    }
    let mut calls: Vec<(&str, &str, &str, Option<i64>)> = Vec::new();
    for m in host.transcript(id).map_or(&[][..], |t| &t.messages[..]) {
        let Message::Assistant(a) = m else { continue };
        for p in &a.parts {
            if let Part::Tool { name, title, input, metadata, .. } = p
                && matches!(name.as_str(), "task" | "subagent")
                && let Some(child) = model::subagent_session(input, metadata)
            {
                let agent = input["agent"].as_str().or(input["subagent_type"].as_str()).unwrap_or("");
                calls.push((child, title, agent, model::tool_ran(metadata)));
            }
        }
    }
    let mut out: Vec<Running> = below
        .into_iter()
        .map(|r| {
            let info = host.session_info(r);
            let call = calls.iter().rev().find(|c| c.0 == r);
            let agent = call.map(|c| c.2).filter(|a| !a.is_empty()).or(info.map(|i| i.agent.as_str())).unwrap_or("");
            let label = call
                .map(|c| c.1.to_owned())
                .filter(|l| !l.is_empty())
                .or(info.map(|i| i.title.clone()))
                .unwrap_or_else(|| r.to_owned());
            let started = if info.is_some_and(|i| i.created > 0) { host.run_started(r, call.and_then(|c| c.3)) } else { 0 };
            Running { session: r.to_owned(), agent: agent.to_owned(), label, started }
        })
        .collect();
    out.sort_by(|a, b| (a.started, &a.session).cmp(&(b.started, &b.session)));
    out
}

/// Closes the activity menu and opens a subagent's session from its parent's timeline.
pub fn open_child(host: &mut dyn SubagentsHost, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
    shells::set_open(host, false);
    host.open_subagent_session(parent, child, title, agent, tx);
}

/// The open session is a subagent's, with the session that started it known.
pub fn has_parent(host: &dyn SubagentsHost) -> bool {
    host.active_session_id().and_then(|id| host.session_info(id)).is_some_and(|s| s.parent.is_some())
}

/// Navigates from a subagent's session back to the session that started it.
pub fn open_parent(host: &mut dyn SubagentsHost, tx: &mut Tx) -> bool {
    let Some(id) = host.active_session_id() else { return false };
    let Some(parent) = host.session_info(id).and_then(|s| s.parent.clone()) else { return false };
    host.navigate_to_session(&parent, tx);
    true
}

/// The child session a call runs in: the one it names, or before it says, the parent's newest
/// child whose title starts with the call's description (`taskSession`).
pub fn child_of<'a>(host: &'a dyn SubagentsHost, parent: &str, named: Option<&'a str>, description: &str) -> Option<&'a str> {
    named.or_else(|| {
        host.sessions()
            .iter()
            .filter(|x| x.parent.as_deref() == Some(parent) && (description.is_empty() || x.title.starts_with(description)))
            .max_by_key(|x| x.created)
            .map(|x| x.id.as_str())
    })
}

/// Toggles the activity capsule popover and notifies the host and popover motion system.
pub fn toggle_activity(host: &mut dyn SubagentsHost, tx: &mut Tx) {
    let next = !host.shells().activity.is_open();
    shells::set_open(host, next);
    let open = host.shells().activity.is_open();
    host.on_activity_toggled(open);
    session_view::popover::motion::toggled(host.knobs(), tx, open);
}

/// Whether the activity capsule currently has any running work to show for session `id`.
pub fn has_capsule(host: &dyn SubagentsHost, id: &str) -> bool {
    let (subs, scr, sh) = counts(host, id);
    subs + scr + sh > 0
}

/// "2 subagents · 1 script · 1 shell" over the composer while anything below the session or one
/// of its background scripts or shells runs; it opens a list. Desktop's capsule
/// (`activity-chips.tsx`): a 28pt hairline capsule filled like the composer (layer 1 in dark,
/// the page in light) with its raised shadow, 10pt insets, the 16pt spinner and the counts in
/// 13pt text, the separator faint (`session.activity.summary`).
pub fn capsule<H: SubagentsHost + 'static>(host: &H, p: &Pal, id: &str) -> Option<El> {
    let (subs, scr, sh) = counts(host, id);
    if subs + scr + sh == 0 {
        return None;
    }
    Some(build_capsule_shell(host, p, id, subs, scr, sh).on_press_tx(|s: &mut H, tx: &mut Tx| toggle_activity(s, tx)))
}

/// The capsule's counts: running subagents, background scripts, and background shells with the
/// one its list shows.
fn counts(host: &dyn SubagentsHost, id: &str) -> (usize, usize, usize) {
    (
        running_subagents(host, id).len(),
        jobs::scripts(host, id).len(),
        shells::running_for(host, id).len() + usize::from(shells::held(host)),
    )
}

fn build_capsule_shell(host: &dyn SubagentsHost, p: &Pal, id: &str, subs: usize, scr: usize, sh: usize) -> El {
    let summary = activity::summary(subs, scr, sh);
    // Leading glyph sits 8 in and 6 from the words, which keep their own 8 apart
    // (`composer-lane.css:62-66`, `session-activity-summary`'s `gap-2`).
    let chip = chip(host, p, "activity-chip", summary.label, host.shells().activity.is_open()).pl(8.0).gap(6.0).child(activity_ring_sized(p, 16.0));
    let mut words = row().key("chip-summary").gap(8.0).center_y();
    let groups = [("count", summary.subagents, subs), ("scripts", summary.scripts, scr), ("shells", summary.shells, sh)];
    for (i, (key, label, n)) in groups.into_iter().filter(|g| g.2 > 0).enumerate() {
        if i > 0 {
            words = words.child(text(summary.between.clone()).key(format!("chip-between-{key}")).font(UI).color(p.faint));
        }
        words = words.child(odometer(format!("chip-{key}-{id}"), label, n as f64, UI, p.text));
    }
    chip.child(words)
}

/// The capsule every pill over the composer shares (`[data-composer-pill]`,
/// `composer-lane.css:2-56`): 32 tall, 10 in, 8 between its parts, 8 pt corners (the Capsule
/// radius knob), filled like the composer and raised (`--v2-elevation-raised`); hover and
/// press lay their overlays on it. While its list is `open` it steps onto the list's layer
/// (layer 1, layer 2 in dark) with no overlay. The Capsule fill and ring knobs thin its
/// surface and hairline.
pub fn chip(host: &dyn SubagentsHost, p: &Pal, key: &str, label: String, open: bool) -> El {
    chip_with_knobs(host.knobs(), p, key, label, open)
}

/// [`chip`] parameterized directly by `knobs`, usable without a full [`SubagentsHost`].
pub fn chip_with_knobs(knobs: &Knobs, p: &Pal, key: &str, label: String, open: bool) -> El {
    let solid = knobs.get(&CAPSULE_FILL);
    let bg = match (open, p.dark) {
        (false, _) => session_view::composer::composer_fill(p),
        (true, false) => p.layer1,
        (true, true) => p.layer2,
    };
    let [far, near, ring, edge] = p.raised;
    let thin = |sh: rune::Shadow| rune::Shadow { color: sh.color.alpha(sh.color.a * solid), ..sh };
    let ring = rune::Shadow { color: p.border_muted.mix(ring.color, knobs.get(&CAPSULE_RING)), ..ring };
    let chip = row()
        .key(key)
        .role("button")
        .label(label)
        .h(session_view::jump::chip_height())
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(knobs.get(&CAPSULE_RADIUS))
        .fill(bg.alpha(bg.a * solid))
        .box_shadow(thin(edge))
        .box_shadow(ring)
        .box_shadow(thin(near))
        .box_shadow(thin(far))
        .hover_motion(INSTANT, INSTANT);
    if open { chip } else { chip.hover(p.hover).press(p.pressed) }
}

/// The capsule's list (`session-activity-panel`): a floating menu above the capsule, start
/// edges aligned, one 28pt row per running subagent with the spinner, the agent in medium, its
/// task in muted text and the run time in 11pt at the end, then one per background shell with
/// the console mark and its command across the agent and task columns. Agents and run times
/// line up in columns; the task takes the rest and fades out where it's cut once the menu
/// reaches 360pt. Hovering a row swaps its run time for a stop button. Past 270pt the rows
/// scroll inside the menu, fading only where rows hide under its edge, and nothing draws
/// outside it. A shell's row opens its own panel in place (`shells_view::panel`).
pub fn panel<H: SubagentsHost + 'static>(host: &H, p: &Pal, id: &str) -> Option<El> {
    if !host.shells().activity.is_open() {
        return None;
    }
    if host.shells().activity.shown().is_some() {
        return shells_view::panel(host, p);
    }
    let layout = prepare_panel_layout(host, id)?;
    let clock = panel_clock(host);
    let mut list = col().key("activity-list").w_full().padding(PANEL_PAD);
    for it in &layout.items {
        let (parent, child, title, agent) = (id.to_owned(), it.session.clone(), it.label.clone(), it.agent.clone());
        let (visible, hover, stop_key) = (child.clone(), child.clone(), child.clone());
        let name = subagent_name(it);
        let ms = (it.started > 0).then(|| clock - it.started);
        let end_el = shells_view::end(host, p, &it.session, activity::Kind::Subagent, ms, layout.time_w);
        list = list.child(
            build_subagent_row_shell(p, it, name, layout.agent_w, end_el)
                .on_layout_tx(move |s: &mut H, _, tx: &mut Tx| prefetch_on_layout(s, &visible, tx))
                .on_hover_change(move |s: &mut H, inside, tx: &mut Tx| prefetch_on_hover(s, &hover, inside, tx))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| open_child_instant(s, &parent, &child, &title, &agent, tx))
                .on_key(move |s: &mut H, ev: &KeyEvent, tx: &mut Tx| shells_view::stop_on_delete(s, &stop_key, activity::Kind::Subagent, ev, tx)),
        );
    }
    for sc in &layout.scripts {
        list = list.child(shells_view::script_row(host, p, sc, layout.span, layout.time_w));
    }
    for sh in &layout.shell_items {
        list = list.child(shells_view::row_of(host, p, sh, layout.span, layout.time_w));
    }
    // `MenuScroll` (`activity-chips.tsx:214`). What it holds comes in with the list, not on fades
    // of its own.
    let rows = session_view::scroller::scroller("activity-scroll")
        .bar(session_view::scroller::Bar::MenuScroll)
        .build::<H>(p, col().w_full().h(layout.h).scroll_edge_fade(12.0).scroll_margin(PANEL_PAD).instant_changes(), list);
    Some(build_panel_frame(host.knobs(), p, layout.natural, layout.label, rows).on_press(|_: &mut H| {}))
}

struct PanelLayout {
    items: Vec<Running>,
    scripts: Vec<jobs::Script>,
    shell_items: Vec<Shell>,
    agent_w: f32,
    time_w: f32,
    span: f32,
    natural: f32,
    h: f32,
    label: String,
}

fn prepare_panel_layout(host: &dyn SubagentsHost, id: &str) -> Option<PanelLayout> {
    let items = running_subagents(host, id);
    let scripts = jobs::scripts(host, id);
    let shell_items: Vec<Shell> = shells::running_for(host, id).into_iter().cloned().collect();
    if items.is_empty() && scripts.is_empty() && shell_items.is_empty() {
        return None;
    }
    let clock = panel_clock(host);
    let time_at = |started: i64| if started > 0 { elapsed(clock - started) } else { String::new() };
    let width = |t: &str, style| rune::text::measure(t, &[], style, None).0;
    let agent_w = items.iter().map(|it| width(&subagent_name(it), UI_MED)).fold(0.0, f32::max);
    let times = items.iter().map(|it| it.started).chain(scripts.iter().map(|sc| sc.started)).chain(shell_items.iter().map(|sh| sh.started));
    let time_w = time_column(times.map(time_at), &width);
    let label_w = items.iter().map(|it| width(&it.label, UI)).fold(0.0, f32::max);
    // A script's or shell's command spans the agent and task columns and the gap between them.
    let commands = scripts.iter().map(|sc| sc.label.as_str()).chain(shell_items.iter().map(|sh| sh.command.as_str()));
    let command_w = commands.map(|c| width(c, shells_view::COMMAND)).fold(0.0, f32::max);
    let span = (agent_w + 6.0 + label_w).max(command_w);
    // Panel inset 4 + hairline 0.5, row inset 4, then the mark, agent and task (or command) and
    // time, 6 apart, with 8 more before the time (`activity-chips.css:9-64`).
    let natural = 2.0 * (PANEL_PAD + 0.5 + 4.0) + 16.0 + 6.0 + span + 14.0 + time_w + if session_view::style::hover_none() { 46.0 } else { 0.0 };
    let n = items.len() + scripts.len() + shell_items.len();
    let h = (n as f32 * panel_row() + 2.0 * PANEL_PAD).min(PANEL_MAX_H);
    let label = activity::summary(items.len(), scripts.len(), shell_items.len()).label;
    Some(PanelLayout { items, scripts, shell_items, agent_w, time_w, span, natural, h, label })
}

fn subagent_name(it: &Running) -> String {
    if it.agent.is_empty() { opencode_i18n::t!("ui.tool.agent.default").to_owned() } else { model::capitalize(&it.agent) }
}

fn build_subagent_row_shell(p: &Pal, it: &Running, name: String, agent_w: f32, end_el: El) -> El {
    row()
        .key(format!("sub-{}", it.session))
        .role("menuitem")
        .label(format!("{name} {}", it.label))
        .w_full()
        .h(panel_row())
        .px(4.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .hover_group()
        .child(activity_ring_sized(p, 16.0))
        .child(text(name).font(UI_MED).color(p.text).w(agent_w))
        .child(text(it.label.clone()).key("label").font(UI).color(p.muted).truncate_fade().grow(1.0).shrink().min_w(0.0))
        .child(end_el)
}

fn build_panel_frame(knobs: &Knobs, p: &Pal, natural: f32, label: String, rows: El) -> El {
    col()
        .key("activity-panel")
        .role("menu")
        .label(label)
        .above("activity-chip", 8.0)
        .w(natural.clamp(240.0, 360.0).ceil())
        .radius(10.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(session_view::popover::motion::transition(knobs, rune::Side::Top, rune::Align::Start))
        .child(rows)
}

fn prefetch_on_layout(host: &mut dyn SubagentsHost, id: &str, tx: &mut Tx) {
    tx.live_only();
    host.prefetch_session(id);
}

fn prefetch_on_hover(host: &mut dyn SubagentsHost, id: &str, inside: bool, tx: &mut Tx) {
    tx.live_only();
    if inside {
        host.prefetch_session(id);
    }
}

fn open_child_instant(host: &mut dyn SubagentsHost, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
    tx.instant();
    open_child(host, parent, child, title, agent, tx);
}

/// The run-time column's width: the longest time shown, its figures tabular so ticking moves
/// nothing until a time gains a unit, as Desktop's grid column is as wide as its longest run
/// time (`activity-chips.css:77-91`); at least the stop button that swaps in for it.
fn time_column(times: impl Iterator<Item = String>, width: &impl Fn(&str, rune::TextStyle) -> f32) -> f32 {
    let widest = |t: &str| width(&t.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect::<String>(), ELAPSED);
    times.map(|t| widest(&t)).fold(20.0, f32::max)
}

pub(crate) const PANEL_PAD: f32 = 4.0;
pub(crate) fn panel_row() -> f32 {
    if session_view::style::hover_none() { 44.0 } else { 28.0 }
}
/// Desktop's `max-height: min(270px, …)`.
const PANEL_MAX_H: f32 = 270.0;

/// The run time in the list: 11pt, tabular figures.
pub(crate) const ELAPSED: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(16.0).features("\"tnum\"");
