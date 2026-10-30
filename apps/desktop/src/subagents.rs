//! Subagents as Desktop shows them: the running-work capsule above the composer
//! (`app/src/session/composer/activity-chips.tsx`) and the status line that takes the
//! composer's place inside a child session (`subagent-bar.tsx`). The call's card in the
//! parent's timeline is `agent_rows_view.rs`.
//!
//! A subagent runs while its child session, or any session below it, runs. A background call's
//! tool part finishes at once, so the call's own status only counts until it names its child.

use rune::{El, Tx, col, el, icon, row, spacer, text};

use opencode_kit::model::{self, Message, Part};
use rune_theme_opencode::icons as ic;

use crate::odometer::odometer;
use crate::palette::Pal;
use crate::view::{INSTANT, UI, UI_MED, UI_SMALL, activity_ring, activity_ring_sized};
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
        self.activity_open = false;
        self.open(child, tx);
    }

    /// From a subagent's session back to the one that started it.
    pub fn open_parent(&mut self, tx: &mut Tx) -> bool {
        let Route::Session(id) = &self.route else { return false };
        let Some(parent) = self.session(id).and_then(|s| s.parent.clone()) else { return false };
        self.open(&parent, tx);
        true
    }
}

/// "12s", "1m 05s", "1h 02m" (`formatElapsed`).
pub fn elapsed(ms: i64) -> String {
    let t = (ms / 1000).max(0);
    match t {
        0..60 => format!("{t}s"),
        60..3600 => format!("{}m {:02}s", t / 60, t % 60),
        _ => format!("{}h {:02}m", t / 3600, t / 60 % 60),
    }
}

/// A run time, "12s", in tabular digits. It changes at once, as Desktop's run times do: a digit
/// that rolls each second smears the number while it's read.
pub fn run_time(key: String, ms: i64, font: rune::TextStyle, color: rune::Color) -> El {
    text(elapsed(ms)).key(key).font(font.features("\"tnum\"")).color(color)
}

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

/// Inside a subagent's session, in the composer's place: how the run is going, the agent and
/// model, and the way back (Esc too).
pub fn bar(s: &App, p: &Pal, id: &str) -> El {
    let info = s.session(id);
    let running = s.subtree_running(id);
    let outcome = info.and_then(|i| i.outcome.as_deref());
    let (glyph, status) = if running {
        (None, Some("Running"))
    } else {
        match outcome {
            Some("succeeded") => (Some(&ic::CHECK_SMALL), Some("Done")),
            Some("failed") => (Some(&ic::CIRCLE_X), Some("Failed")),
            Some("interrupted") => (Some(&ic::XMARK_SMALL), Some("Interrupted")),
            _ => (None, None),
        }
    };
    // "Running · 12s · General · Claude Opus": the status and its run time, then
    // the rest, which truncates.
    let mut head: Option<(&str, Option<i64>)> = None;
    if let (Some(st), Some(i)) = (status, info) {
        let end = if running { s.run_clock() } else { i.idle.unwrap_or(i.updated) };
        head = Some((st, (i.created > 0).then(|| end - i.created)));
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = info.map(|i| i.agent.as_str()).filter(|a| !a.is_empty()) {
        parts.push(model::capitalize(a));
    }
    if let Some(m) = info.map(|i| &i.model).filter(|m| !m.id.is_empty()) {
        let name = s.pickers.model_name(m);
        let variant = if m.variant.is_empty() || m.variant == "default" { String::new() } else { format!(" {}", model::capitalize(&m.variant)) };
        parts.push(format!("{name}{variant}"));
    }
    let mut line = row().key("subagent-bar").w_full().max_w(crate::view::CONTENT_W).h(32.0).pl(12.0).pr(6.0).gap(8.0).center_y();
    if status.is_some() {
        let mark = el().key("status-icon").size(16.0, 16.0).center();
        line = line.child(match glyph {
            Some(g) => mark.child(icon(g, 16.0).color(p.icon_faint)),
            None => mark.child(activity_ring(p)),
        });
    }
    let mut summary = row().key("summary").shrink().min_w(0.0).center_y();
    let dot = |first: bool| if first { "" } else { " · " };
    if let Some((st, ms)) = head {
        summary = summary.child(text(st.to_owned()).key("status").font(UI_SMALL).color(p.faint));
        if let Some(ms) = ms {
            summary = summary
                .child(text(" · ").key("dot").font(UI_SMALL).color(p.faint))
                .child(run_time(format!("bar-time-{id}"), ms, UI_SMALL, p.faint));
        }
    }
    if !parts.is_empty() {
        let rest = format!("{}{}", dot(head.is_none()), parts.join(" · "));
        summary = summary.child(text(rest).key("rest").font(UI_SMALL).color(p.faint).truncate_fade());
    }
    line = line
        .child(summary)
        .child(spacer())
        .child(
            row()
                .key("back")
                .role("button")
                .label("Main session")
                .h(24.0)
                .px(6.0)
                .gap(6.0)
                .center_y()
                .radius(6.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.open_parent(tx);
                })
                .child(text("Main session").font(UI_SMALL).color(p.faint))
                .child(row().key("esc").h(16.0).px(4.0).center().radius(4.0).fill(p.layer2).child(text("esc").font(UI_SMALL).color(p.faint))),
        );
    col().key("dock").w_full().center().px(24.0).pb(10.0).child(line)
}

/// "2 subagents" over the composer while anything below the session runs; it opens a list.
/// Desktop's capsule (`activity-chips.tsx`): a 28pt hairline capsule filled like the composer
/// (layer 1 in dark, the page in light) with its raised shadow, 10pt insets, the 16pt spinner and
/// the count in 13pt text. The fill's strength is the "Capsule fill" knob.
pub fn capsule(s: &App, p: &Pal, id: &str) -> Option<El> {
    let n = s.running_subagents(id).len();
    if n == 0 {
        return None;
    }
    let label = format!("{n} subagent{}", if n == 1 { "" } else { "s" });
    let bg = crate::view::composer_fill(p);
    let mut chip = row()
        .key("activity-chip")
        .role("button")
        .label(label.clone())
        .h(28.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(s.knobs.get(&crate::knobs::CAPSULE_RADIUS))
        .fill(bg.alpha(s.knobs.get(&crate::knobs::CAPSULE_FILL)))
        // Desktop's `--v2-elevation-raised` rings the muted border with the composer's hairline.
        .border(0.5, p.border_muted.mix(p.ring, s.knobs.get(&crate::knobs::CAPSULE_RING)))
        .shadow(1.0, 2.0, p.shadow.alpha(p.shadow.a * s.knobs.get(&crate::knobs::CAPSULE_FILL)))
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.activity_open = !s.activity_open;
        })
        .child(activity_ring_sized(p, 16.0))
        .child(odometer(format!("chip-count-{id}"), label, n as f64, UI, p.text));
    if s.activity_open {
        chip = chip.fill(p.pressed);
    }
    Some(chip)
}

/// The capsule's list (`session-activity-panel`): a floating menu above the capsule, start
/// edges aligned, one 28pt row per running subagent with the spinner, the agent in medium, its
/// task in muted text and the run time in 11pt at the end. Agents and run times line up in
/// columns; the task takes the rest and fades out where it's cut once the menu reaches 360pt.
/// Past 270pt the rows scroll inside the menu, fading only where rows hide under its edge, and
/// nothing draws outside it.
pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    let items = s.running_subagents(id);
    if !s.activity_open || items.is_empty() {
        return None;
    }
    let name_of = |it: &Running| if it.agent.is_empty() { "Agent".to_owned() } else { model::capitalize(&it.agent) };
    let ms_of = |it: &Running| (it.started > 0).then(|| s.run_clock() - it.started);
    let time_of = |it: &Running| ms_of(it).map(elapsed).unwrap_or_default();
    let width = |t: &str, style| rune::text::measure(t, &[], style, None).0;
    let agent_w = items.iter().map(|it| width(&name_of(it), UI_MED)).fold(0.0, f32::max);
    let time_w = time_column(items.iter().map(time_of), &width);
    let label_w = items.iter().map(|it| width(&it.label, UI)).fold(0.0, f32::max);
    // Panel inset 4 + hairline 0.5, row inset 6, then spinner, agent, task and time, 8 apart,
    // with 8 more before the time.
    let natural = 2.0 * (PANEL_PAD + 0.5 + 6.0) + 16.0 + 8.0 + agent_w + 8.0 + label_w + 16.0 + time_w;
    let mut list = col().key("activity-list").w_full().padding(PANEL_PAD);
    for it in &items {
        let (parent, child, title, agent) = (id.to_owned(), it.session.clone(), it.label.clone(), it.agent.clone());
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
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.open_child(&parent, &child, &title, &agent, tx);
                })
                .child(activity_ring_sized(p, 16.0))
                .child(text(name).font(UI_MED).color(p.text).w(agent_w))
                .child(text(it.label.clone()).key("label").font(UI).color(p.muted).truncate_fade().grow(1.0).shrink().min_w(0.0))
                .child(row().key("time").w(time_w).ml(8.0).justify_end().children(ms_of(it).map(|ms| run_time(format!("sub-time-{}", it.session), ms, ELAPSED, p.muted)))),
        );
    }
    let h = (items.len() as f32 * PANEL_ROW + 2.0 * PANEL_PAD).min(PANEL_MAX_H);
    let rows = col()
        .key("activity-scroll")
        .w_full()
        .h(h)
        .scroll(rune::ScrollSpec { stick_bottom: false })
        .scroll_edge_fade(12.0)
        .scroll_margin(PANEL_PAD)
        .child(list);
    let panel = col()
        .key("activity-panel")
        .role("menu")
        .label("Subagents")
        .above("activity-chip", 8.0)
        .w(natural.clamp(240.0, 360.0).ceil())
        .radius(10.0)
        .fill(p.layer1)
        // --v2-elevation-floating: two drops and a hairline ring.
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, p.shadow_far)
        .shadow(4.0, 8.0, p.shadow)
        .clip_soft(0.0)
        .transition(rune::Transition::none())
        .on_press(|_: &mut App| {})
        .child(rows);
    Some(panel)
}

/// The run-time column's width: room for "59m 59s" at least, and for the widest time shown with
/// every digit at its widest, so ticking never moves the tasks beside it.
fn time_column(times: impl Iterator<Item = String>, width: &impl Fn(&str, rune::TextStyle) -> f32) -> f32 {
    let widest = |t: &str| width(&t.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect::<String>(), ELAPSED);
    times.map(|t| widest(&t)).fold(widest("00m 00s"), f32::max).ceil()
}

const PANEL_PAD: f32 = 4.0;
const PANEL_ROW: f32 = 28.0;
/// Desktop's `max-height: min(270px, …)`.
const PANEL_MAX_H: f32 = 270.0;

/// The run time in the list: 11pt, tabular figures.
const ELAPSED: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(16.0).features("\"tnum\"");

#[cfg(test)]
mod tests {
    use rune::Runtime;

    use crate::fixture_ids::{MAIN, CHILD as REVIEW};
    use crate::fixture_runtime as rt;
    use super::*;

    fn has(rt: &Runtime<App>, path: &str, key: &str) -> bool {
        rt.elements().into_iter().any(|e| !e.exiting && e.path.contains(path) && e.key.as_deref() == Some(key))
    }

    fn press(rt: &mut Runtime<App>, key: &str) {
        // Hits come from the last paint: one more frame so what just appeared takes presses.
        rt.frame(1.0 / 120.0);
        // Scroll the timeline until it's on screen.
        for _ in 0..40 {
            match rt.find(key) {
                Some((_, y)) if y < 60.0 => rt.wheel(640.0, 300.0, 120.0),
                _ => break,
            }
            rt.frame(1.0 / 120.0);
        }
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
    }

    #[test]
    fn elapsed_pads_the_trailing_unit() {
        assert_eq!(elapsed(12_400), "12s");
        assert_eq!(elapsed(65_000), "1m 05s");
        assert_eq!(elapsed(3_720_000), "1h 02m");
    }

    #[test]
    fn a_subagent_card_shows_live_status_and_opens_its_session() {
        let mut rt = rt();
        let card = "a-call_subagent_";
        // Finished: a ✓, and the call names its child.
        assert!(has(&rt, card, "done"));
        assert!(!has(&rt, card, "spin"));
        rt.update(|s, _| {
            s.running.insert(REVIEW.into());
        });
        rt.frame(1.0 / 120.0);
        assert!(has(&rt, card, "spin"), "the child runs, so the card does");
        assert_eq!(rt.state.tab_status(MAIN), Some(crate::TabStatus::Busy));
        assert!(has(&rt, "", "activity-chip"));

        press(&mut rt, "agent-line");
        assert_eq!(rt.state.route, Route::Session(REVIEW.into()));
        // Still the parent's tab, and the composer gives way to the subagent's status line.
        assert!(!rt.state.tabs.iter().any(|t| t.id == REVIEW));
        assert!(has(&rt, "", "subagent-bar"));
        assert!(!has(&rt, "", "prompt"));

        press(&mut rt, "back");
        assert_eq!(rt.state.route, Route::Session(MAIN.into()));
    }

    #[test]
    fn the_capsule_lists_running_subagents() {
        let mut rt = rt();
        assert!(!has(&rt, "", "activity-chip"));
        rt.update(|s, _| {
            s.running.insert(REVIEW.into());
        });
        rt.frame(1.0 / 120.0);
        assert_eq!(
            rt.state.running_subagents(MAIN),
            // Dated from the call to it (its part's time), not the child's creation.
            vec![Running { session: REVIEW.into(), agent: "general".into(), label: "Review the reconnect change".into(), started: 1_792_658_360_500 }]
        );
        press(&mut rt, "activity-chip");
        assert!(has(&rt, "", &format!("sub-{REVIEW}")));
        press(&mut rt, &format!("sub-{REVIEW}"));
        assert_eq!(rt.state.route, Route::Session(REVIEW.into()));
        assert!(!rt.state.activity_open);
    }

    #[test]
    fn a_woken_subagent_is_dated_from_its_latest_run() {
        let mut rt = rt();
        let hour = 3_600_000;
        rt.update(|s, _| {
            s.running.insert(REVIEW.into());
            let now = s.now_ms;
            let info = s.sessions.iter_mut().find(|i| i.id == REVIEW).unwrap();
            // Made two hours ago, called, idle ten minutes ago, running again since.
            info.created = now - 2 * hour;
            info.idle = Some(now - 600_000);
        });
        assert_eq!(rt.state.running_subagents(MAIN)[0].started, rt.state.now_ms - 600_000, "from when it last went idle");
        rt.update(|s, _| {
            s.agent_rows.started.insert(REVIEW.into(), s.now_ms - 90_000);
        });
        assert_eq!(rt.state.running_subagents(MAIN)[0].started, rt.state.now_ms - 90_000, "from the start we saw");
        let call = rt.state.now_ms - 30_000;
        assert_eq!(rt.state.run_started(REVIEW, Some(call)), call, "from the latest call to it");
    }

    #[test]
    fn the_list_scrolls_inside_its_menu() {
        let mut rt = rt();
        rt.update(|s, tx| {
            crate::capsule_stories::run_thirty(s);
            s.activity_open = true;
            tx.instant();
        });
        rt.frame(1.0 / 120.0);
        let rect = |key: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect).unwrap();
        let menu = rect("activity-panel");
        assert_eq!(menu[3], PANEL_MAX_H, "it stops at 270");
        assert!(rect("activity-list")[3] > 30.0 * PANEL_ROW, "and the rows scroll inside it");
        // Every run time sits in a column of one width, ending at the same edge.
        let times: Vec<_> = rt.elements().into_iter().filter(|e| e.path.contains("activity-list") && e.key.as_deref() == Some("time")).map(|e| (e.rect[0], e.rect[2])).collect();
        assert_eq!(times.len(), 30);
        assert!(times.iter().all(|t| *t == times[0]), "{times:?}");
    }

    #[test]
    fn the_capsule_is_filled_like_the_composer_and_the_knob_clears_it() {
        let mut rt = rt();
        rt.update(|s, _| {
            s.running.insert(REVIEW.into());
        });
        rt.frame(1.0 / 120.0);
        let fill = |rt: &Runtime<App>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k)).unwrap().fill;
        let (chip, composer) = (fill(&rt, "activity-chip"), fill(&rt, "composer"));
        assert!(chip.a > 0.99 && (chip.r - composer.r).abs() < 1e-3 && (chip.g - composer.g).abs() < 1e-3, "{chip:?} vs {composer:?}");
        rt.key(rune::KeyEvent { key: rune::Key::Command('k'), mods: rune::Mods { cmd: true, shift: true, ..Default::default() } });
        rt.frame(1.0 / 120.0);
        assert!(rt.state.knobs.open);
        rt.update(|s, tx| {
            tx.instant();
            s.knobs.set(&crate::knobs::CAPSULE_FILL, 0.0);
        });
        for _ in 0..30 {
            rt.frame(1.0 / 120.0);
        }
        let after = fill(&rt, "activity-chip");
        assert!(after.a < 1e-3, "{after:?} knob {}", rt.state.knobs.get(&crate::knobs::CAPSULE_FILL));
    }

    #[test]
    fn esc_in_a_subagent_goes_back_to_its_parent() {
        let mut rt = rt();
        rt.update(|s, tx| s.open(REVIEW, tx));
        rt.frame(1.0 / 120.0);
        rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() });
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.route, Route::Session(MAIN.into()));
    }

    #[test]
    fn an_unlisted_child_opens_under_its_parent() {
        let mut rt = rt();
        rt.update(|s, tx| s.open_child(MAIN, "ses_unlisted", "Probe the cache", "explore", tx));
        let s = &rt.state;
        assert_eq!(s.route, Route::Session("ses_unlisted".into()));
        assert_eq!(s.root_of("ses_unlisted"), MAIN);
        assert!(!s.tabs.iter().any(|t| t.id == "ses_unlisted"));
    }

    #[test]
    fn the_panel_follows_the_text_size() {
        let mut rt = rt();
        rt.update(|s, tx| {
            s.running.insert(REVIEW.into());
            s.activity_open = true;
            tx.instant();
        });
        rt.frame(1.0 / 60.0);
        let small = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("activity-panel")).unwrap();
        assert!((small.scale - 1.0).abs() < 1e-3);
        rt.update(|s, tx| {
            s.zoom = 1.6;
            tx.instant();
        });
        rt.frame(1.0 / 60.0);
        let big = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("activity-panel")).unwrap();
        assert!((big.scale - 1.6).abs() < 1e-3, "scales with zoom: {}", big.scale);
        assert!((big.rect[3] / small.rect[3] - 1.6).abs() < 0.01, "height {} vs {}", big.rect[3], small.rect[3]);
    }
}
