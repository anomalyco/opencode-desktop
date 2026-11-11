//! Subagents as Desktop shows them: the running-work capsule above the composer
//! (`app/src/session/composer/activity-chips.tsx`). A subagent's own page answers it
//! (`reply.rs`). The call's card in the parent's timeline is `agent_rows_view.rs`.
//!
//! A subagent runs while its child session, or any session below it, runs. A background call's
//! tool part finishes at once, so the call's own status only counts until it names its child.

pub mod activity;
pub mod reply;
mod shell_follow;
mod shell_tail;
pub mod shells;
mod shells_view;
mod stories;

use rune::{El, Tx, col, row, text};

use opencode_kit::model::{self, Message, Part};

use crate::odometer::odometer;
use crate::palette::{Elevate as _, Pal};
use crate::view::{INSTANT, UI, UI_MED, activity_ring_sized};
use crate::{App, Route};

/// One running subagent of a session, for the capsule's list.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub session: String,
    pub agent: String,
    pub label: String,
    pub started: i64,
}

impl App {
    /// Whether `id` is `ancestor` or a session below it.
    pub fn descends(&self, id: &str, ancestor: &str) -> bool {
        self.ancestors(id).take(32).any(|cur| cur == ancestor)
    }

    /// Whether `id` or anything below it is running.
    pub fn subtree_running(&self, id: &str) -> bool {
        self.running.iter().any(|r| self.descends(r, id))
    }

    /// The sessions running below `id`, oldest first, named and dated by their latest call in
    /// `id`'s transcript when there is one.
    pub fn running_subagents(&self, id: &str) -> Vec<Running> {
        let below: Vec<&String> = self.running.iter().filter(|r| r.as_str() != id && self.descends(r, id)).collect();
        if below.is_empty() {
            return Vec::new();
        }
        let mut calls: Vec<(&str, &str, &str, Option<i64>)> = Vec::new();
        for m in self.transcripts.get(id).map_or(&[][..], |t| &t.messages[..]) {
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
                let info = self.session(r);
                let call = calls.iter().rev().find(|c| c.0 == r);
                let agent = call.map(|c| c.2).filter(|a| !a.is_empty()).or(info.map(|i| i.agent.as_str())).unwrap_or("");
                let label = call.map(|c| c.1.to_owned()).filter(|l| !l.is_empty()).or(info.map(|i| i.title.clone())).unwrap_or_else(|| r.clone());
                let started = if info.is_some_and(|i| i.created > 0) { self.run_started(r, call.and_then(|c| c.3)) } else { 0 };
                Running { session: r.clone(), agent: agent.to_owned(), label, started }
            })
            .collect();
        out.sort_by(|a, b| (a.started, &a.session).cmp(&(b.started, &b.session)));
        out
    }

    /// Opens a subagent's session from its parent's timeline. A child the session list hasn't
    /// loaded yet gets a placeholder so it opens under the parent's tab, then its real info.
    pub fn open_child(&mut self, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
        if self.session(child).is_none() {
            let from = self.session(parent).cloned().unwrap_or_default();
            self.sessions.push(model::SessionInfo {
                id: child.to_owned(),
                parent: Some(parent.to_owned()),
                project: from.project,
                directory: from.directory,
                title: title.to_owned(),
                agent: agent.to_owned(),
                ..Default::default()
            });
            self.fetch_info(child);
        }
        shells::set_open(self, false);
        self.open(child, tx);
    }

    /// From a subagent's session back to the one that started it.
    /// The open session is a subagent's, with the session that started it known.
    pub fn has_parent(&self) -> bool {
        matches!(&self.route, Route::Session { id, .. } if self.session(id).is_some_and(|s| s.parent.is_some()))
    }

    pub fn open_parent(&mut self, tx: &mut Tx) -> bool {
        let Route::Session { id, .. } = &self.route else { return false };
        let Some(parent) = self.session(id).and_then(|s| s.parent.clone()) else { return false };
        self.open(&parent, tx);
        true
    }
}

pub use session_view::subagent_card::{elapsed, run_time};

/// The child session a call runs in: the one it names, or before it says, the parent's newest
/// child whose title starts with the call's description (`taskSession`).
pub fn child_of<'a>(s: &'a App, parent: &str, named: Option<&'a str>, description: &str) -> Option<&'a str> {
    named.or_else(|| {
        s.sessions
            .iter()
            .filter(|x| x.parent.as_deref() == Some(parent) && (description.is_empty() || x.title.starts_with(description)))
            .max_by_key(|x| x.created)
            .map(|x| x.id.as_str())
    })
}

/// "2 subagents · 1 shell" over the composer while anything below the session or one of its
/// background shells runs; it opens a list. Desktop's capsule (`activity-chips.tsx`): a 28pt
/// hairline capsule filled like the composer (layer 1 in dark, the page in light) with its
/// raised shadow, 10pt insets, the 16pt spinner and the counts in 13pt text, the separator faint
/// (`session.activity.summary`). The fill's strength is the "Capsule fill" knob.
pub fn capsule(s: &App, p: &Pal, id: &str) -> Option<El> {
    let (subs, sh) = counts(s, id);
    if subs + sh == 0 {
        return None;
    }
    let subs_label = opencode_i18n::plural!("session.activity.subagents", subs);
    let shells_label = opencode_i18n::plural!("session.activity.shells", sh);
    // The summary's words between its two counts, in the language's order.
    let summary = opencode_i18n::t!("session.activity.summary", subagents = "\u{1}", shells = "\u{2}");
    let between = summary.split(['\u{1}', '\u{2}']).nth(1).unwrap_or("").trim().to_owned();
    let label = match (subs, sh) {
        (0, _) => shells_label.clone(),
        (_, 0) => subs_label.clone(),
        _ => opencode_i18n::t!("session.activity.summary", subagents = subs_label.clone(), shells = shells_label.clone()).to_string(),
    };
    let mut chip = chip(s, p, "activity-chip", label, s.shells.activity.is_open())
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            shells::set_open(s, !s.shells.activity.is_open());
            s.todo_open = false;
            crate::popover::motion::toggled(&s.knobs, tx, s.shells.activity.is_open());
        })
        .child(activity_ring_sized(p, 16.0));
    if subs > 0 {
        chip = chip.child(odometer(format!("chip-count-{id}"), subs_label, subs as f64, UI, p.text));
    }
    if subs > 0 && sh > 0 {
        chip = chip.child(text(between).key("chip-between").font(UI).color(p.faint));
    }
    if sh > 0 {
        chip = chip.child(odometer(format!("chip-shells-{id}"), shells_label, sh as f64, UI, p.text));
    }
    Some(chip)
}

/// The capsule's counts: running subagents, and background shells with the one its list shows.
fn counts(s: &App, id: &str) -> (usize, usize) {
    (s.running_subagents(id).len(), shells::running_for(s, id).len() + usize::from(shells::held(s)))
}

/// The capsule every pill over the composer shares (`[data-composer-pill]`,
/// `composer-lane.css:2-56`): 32 tall, 10 in, 8 between its parts, 8 pt corners (the Capsule
/// radius knob), filled like the composer and raised (`--v2-elevation-raised`); hover and
/// press lay their overlays on it. While its list is `open` it steps onto the list's layer
/// (layer 1, layer 2 in dark) with no overlay. The Capsule fill and ring knobs thin its
/// surface and hairline.
pub fn chip(s: &App, p: &Pal, key: &str, label: String, open: bool) -> El {
    let solid = s.knobs.get(&crate::knobs::CAPSULE_FILL);
    let bg = match (open, p.dark) {
        (false, _) => crate::view::composer_fill(p),
        (true, false) => p.layer1,
        (true, true) => p.layer2,
    };
    let [far, near, ring, edge] = p.raised;
    let thin = |sh: rune::Shadow| rune::Shadow { color: sh.color.alpha(sh.color.a * solid), ..sh };
    let ring = rune::Shadow { color: p.border_muted.mix(ring.color, s.knobs.get(&crate::knobs::CAPSULE_RING)), ..ring };
    let chip = row()
        .key(key)
        .role("button")
        .label(label)
        .h(CHIP_H)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(s.knobs.get(&crate::knobs::CAPSULE_RADIUS))
        .fill(bg.alpha(bg.a * solid))
        .box_shadow(thin(edge))
        .box_shadow(ring)
        .box_shadow(thin(near))
        .box_shadow(thin(far))
        .hover_motion(INSTANT, INSTANT);
    if open { chip } else { chip.hover(p.hover).press(p.pressed) }
}

/// A composer pill's height (`composer-lane.css:7`).
pub const CHIP_H: f32 = session_view::jump::CHIP_H;

/// The capsule's list (`session-activity-panel`): a floating menu above the capsule, start
/// edges aligned, one 28pt row per running subagent with the spinner, the agent in medium, its
/// task in muted text and the run time in 11pt at the end, then one per background shell with
/// the console mark and its command across the agent and task columns. Agents and run times
/// line up in columns; the task takes the rest and fades out where it's cut once the menu
/// reaches 360pt. Hovering a row swaps its run time for a stop button. Past 270pt the rows
/// scroll inside the menu, fading only where rows hide under its edge, and nothing draws
/// outside it. A shell's row opens its own panel in place (`shells_view::panel`).
pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    if !s.shells.activity.is_open() {
        return None;
    }
    if s.shells.activity.shown().is_some() {
        return shells_view::panel(s, p);
    }
    let items = s.running_subagents(id);
    let shell_items = shells::running_for(s, id);
    if items.is_empty() && shell_items.is_empty() {
        return None;
    }
    let name_of = |it: &Running| if it.agent.is_empty() { opencode_i18n::t!("ui.tool.agent.default").to_owned() } else { model::capitalize(&it.agent) };
    let ms_of = |it: &Running| (it.started > 0).then(|| s.run_clock() - it.started);
    let time_of = |it: &Running| ms_of(it).map(elapsed).unwrap_or_default();
    let width = |t: &str, style| rune::text::measure(t, &[], style, None).0;
    let agent_w = items.iter().map(|it| width(&name_of(it), UI_MED)).fold(0.0, f32::max);
    let shell_times = shell_items.iter().map(|sh| (sh.started > 0).then(|| elapsed(s.run_clock() - sh.started)).unwrap_or_default());
    let time_w = time_column(items.iter().map(time_of).chain(shell_times), &width);
    let label_w = items.iter().map(|it| width(&it.label, UI)).fold(0.0, f32::max);
    // A shell's command spans the agent and task columns and the gap between them.
    let command_w = shell_items.iter().map(|sh| width(&sh.command, shells_view::COMMAND)).fold(0.0, f32::max);
    let span = (agent_w + 8.0 + label_w).max(command_w);
    // Panel inset 4 + hairline 0.5, row inset 6, then the mark, agent and task (or command) and
    // time, 8 apart, with 8 more before the time.
    let natural = 2.0 * (PANEL_PAD + 0.5 + 6.0) + 16.0 + 8.0 + span + 16.0 + time_w;
    let mut list = col().key("activity-list").w_full().padding(PANEL_PAD);
    for it in &items {
        let (parent, child, title, agent) = (id.to_owned(), it.session.clone(), it.label.clone(), it.agent.clone());
        let (visible, hover) = (child.clone(), child.clone());
        let name = name_of(it);
        list = list.child(
            row()
                .key(format!("sub-{}", it.session))
                .role("menuitem")
                .label(format!("{name} {}", it.label))
                .w_full()
                .h(PANEL_ROW)
                .px(6.0)
                .gap(8.0)
                .center_y()
                .radius(4.0)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .hover_group()
                .on_layout_tx(move |s: &mut App, _, tx| {
                    tx.live_only();
                    s.prefetch_session(&visible);
                })
                .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                    tx.live_only();
                    if inside { s.prefetch_session(&hover); }
                })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.open_child(&parent, &child, &title, &agent, tx);
                })
                .child(activity_ring_sized(p, 16.0))
                .child(text(name).font(UI_MED).color(p.text).w(agent_w))
                .child(text(it.label.clone()).key("label").font(UI).color(p.muted).truncate_fade().grow(1.0).shrink().min_w(0.0))
                .child(shells_view::end(s, p, &it.session, true, ms_of(it), time_w)),
        );
    }
    for sh in &shell_items {
        list = list.child(shells_view::row_of(s, p, sh, span, time_w));
    }
    let n = items.len() + shell_items.len();
    let h = (n as f32 * PANEL_ROW + 2.0 * PANEL_PAD).min(PANEL_MAX_H);
    // `MenuScroll` (`activity-chips.tsx:214`). What it holds comes in with the list, not on fades
    // of its own.
    let rows = session_view::scroller::scroller("activity-scroll")
        .bar(session_view::scroller::Bar::MenuScroll)
        .build::<App>(p, col().w_full().h(h).scroll_edge_fade(12.0).scroll_margin(PANEL_PAD).instant_changes(), list);
    let label = if items.is_empty() { opencode_i18n::t!("session.activity.section.shells") } else { opencode_i18n::t!("session.activity.section.subagents") };
    let panel = col()
        .key("activity-panel")
        .role("menu")
        .label(label)
        .above("activity-chip", 8.0)
        .w(natural.clamp(240.0, 360.0).ceil())
        .radius(10.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(crate::popover::motion::transition(&s.knobs, rune::Side::Top, rune::Align::Start))
        .on_press(|_: &mut App| {})
        .child(rows);
    Some(panel)
}
/// The run-time column's width: the longest time shown, its figures tabular so ticking moves
/// nothing until a time gains a unit, as Desktop's grid column is as wide as its longest run
/// time (`activity-chips.css:77-91`); at least the stop button that swaps in for it.
fn time_column(times: impl Iterator<Item = String>, width: &impl Fn(&str, rune::TextStyle) -> f32) -> f32 {
    let widest = |t: &str| width(&t.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect::<String>(), ELAPSED);
    times.map(|t| widest(&t)).fold(20.0, f32::max)
}

pub(crate) const PANEL_PAD: f32 = 4.0;
pub(crate) const PANEL_ROW: f32 = 28.0;
/// Desktop's `max-height: min(270px, …)`.
const PANEL_MAX_H: f32 = 270.0;

/// The run time in the list: 11pt, tabular figures.
pub(crate) const ELAPSED: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(16.0).features("\"tnum\"");

