//! Subagents as Desktop shows them: the call's card in the parent's timeline
//! (`session-ui/src/tools/tool-renderer.tsx`, "subagent"), the running-work capsule above the
//! composer (`app/src/session/composer/activity-chips.tsx`), and the status line that takes the
//! composer's place inside a child session (`subagent-bar.tsx`).
//!
//! A subagent runs while its child session, or any session below it, runs. A background call's
//! tool part finishes at once, so the call's own status only counts until it names its child.

use rune::{Anim, El, Tx, col, el, icon, row, spacer, text};

use opencode_kit::model::{self, Message, Part, ToolStatus};
use rune_theme_opencode::icons as ic;

use crate::palette::Pal;
use crate::view::{UI, UI_MED, UI_SMALL, activity_ring, activity_ring_on};
use crate::{App, Route};

const INSTANT: Anim = Anim::INSTANT;

/// One running subagent of a session, for the capsule's list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Running {
    pub session: String,
    pub agent: String,
    pub label: String,
    pub started: i64,
}

impl App {
    /// Whether `id` is `ancestor` or a session below it.
    pub fn descends(&self, id: &str, ancestor: &str) -> bool {
        let mut cur = id;
        for _ in 0..32 {
            if cur == ancestor {
                return true;
            }
            match self.session(cur).and_then(|s| s.parent.as_deref()) {
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    }

    /// Whether `id` or anything below it is running.
    pub fn subtree_running(&self, id: &str) -> bool {
        self.running.iter().any(|r| self.descends(r, id))
    }

    /// The sessions running below `id`, oldest first, named by their latest call in `id`'s
    /// transcript when there is one.
    pub fn running_subagents(&self, id: &str) -> Vec<Running> {
        let running = self.running.iter().map(String::as_str).filter(|r| *r != id && self.descends(r, id));
        self.subagents_named(id, running)
    }

    /// `children` of `id`, oldest first, named as [`App::running_subagents`] names them.
    pub fn subagents_named<'a>(&self, id: &str, children: impl Iterator<Item = &'a str>) -> Vec<Running> {
        let mut calls: Vec<(&str, &str, &str)> = Vec::new();
        for m in self.transcripts.get(id).map_or(&[][..], |t| &t.messages[..]) {
            let Message::Assistant(a) = m else { continue };
            for p in &a.parts {
                if let Part::Tool { name, title, input, metadata, .. } = p
                    && matches!(name.as_str(), "task" | "subagent")
                    && let Some(child) = model::subagent_session(input, metadata)
                {
                    calls.push((child, title, input["agent"].as_str().or(input["subagent_type"].as_str()).unwrap_or("")));
                }
            }
        }
        let mut out: Vec<Running> = children
            .map(|r| {
                let info = self.session(r);
                let call = calls.iter().rev().find(|c| c.0 == r);
                let agent = call.map(|c| c.2).filter(|a| !a.is_empty()).or(info.map(|i| i.agent.as_str())).unwrap_or("");
                let label = call.map(|c| c.1.to_owned()).filter(|l| !l.is_empty()).or(info.map(|i| i.title.clone())).unwrap_or_else(|| r.to_owned());
                Running { session: r.to_owned(), agent: agent.to_owned(), label, started: info.map_or(0, |i| i.created) }
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

/// The call's card: a spinner while it runs, the agent, the description, and a chevron on
/// hover when there's a session to open.
#[allow(clippy::too_many_arguments)]
pub fn card(s: &App, p: &Pal, parent: &str, agent: &str, title: &str, status: &ToolStatus, named: Option<&str>, background: bool) -> El {
    if *status == ToolStatus::Pending {
        // The call is still being written: Desktop's "Delegating agent…" pill.
        return row().w_full().child(
            row()
                .key("delegating")
                .h(36.0)
                .px(12.0)
                .gap(8.0)
                .center_y()
                .radius(8.0)
                .fill(p.layer2)
                .child(activity_ring(p))
                .child(text("Delegating agent…").font(UI_MED).color(p.text)),
        );
    }
    let child = child_of(s, parent, named, title);
    let running = *status == ToolStatus::Running || child.is_some_and(|c| s.subtree_running(c));
    let subtitle = if background && !title.is_empty() { format!("{title} (background)") } else { title.to_owned() };
    let mut surface = row()
        .key("chip")
        .role("button")
        .label(format!("{} {subtitle}", model::capitalize(agent)))
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .radius(8.0)
        .fill(p.layer2)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        // The spinner leaving as the run ends slides the words over rather than jumping them.
        .animate_layout();
    if running {
        surface = surface.child(activity_ring_on(p, p.layer2, 14.0).key("spinner"));
    }
    surface = surface
        .child(text(model::capitalize(agent)).key("agent").font(UI_MED.lh(20.0)).color(p.text))
        .child(fit(s, text(subtitle).key("subtitle").font(UI.lh(20.0)).color(p.text)).shrink())
        .child(spacer());
    if let Some(c) = child {
        let (parent, c, title, agent) = (parent.to_owned(), c.to_owned(), title.to_owned(), agent.to_owned());
        surface = surface
            .child(icon(&ic::CHEVRON_RIGHT, 16.0).key("open").color(p.icon_faint).group_opacity(0.0, 1.0))
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.open_child(&parent, &c, &title, &agent, tx);
            });
    }
    row().w_full().child(surface)
}

/// One line cut as Desktop cuts it, or with a soft fade in the live Subagents styles.
fn fit(s: &App, t: El) -> El {
    if crate::agents::style(s) == crate::agents::Style::Desktop { t.truncate() } else { t.truncate_fade() }
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
    let mut parts: Vec<String> = Vec::new();
    if let (Some(st), Some(i)) = (status, info) {
        parts.push(st.to_owned());
        if i.created > 0 {
            let end = if running { s.now_ms } else { i.idle.unwrap_or(i.updated) };
            parts.push(elapsed(end - i.created));
        }
    }
    if let Some(a) = info.map(|i| i.agent.as_str()).filter(|a| !a.is_empty()) {
        parts.push(model::capitalize(a));
    }
    if let Some(m) = info.map(|i| &i.model).filter(|m| !m.id.is_empty()) {
        let name = s.pickers.models.iter().find(|x| x.id == m.id && x.provider == m.provider).map_or_else(|| m.display(), |x| x.name.clone());
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
    line = line
        .child(text(parts.join(" · ")).key("summary").font(UI_SMALL).color(p.faint).truncate().shrink())
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
    let mut chip = row()
        .key("activity-chip")
        .role("button")
        .label(label.clone())
        .h(28.0)
        .px(10.0)
        .gap(8.0)
        .center_y()
        .radius(s.knobs.get(&crate::knobs::CAPSULE_RADIUS))
        .fill(if p.dark { p.layer1 } else { p.bg_base }.alpha(s.knobs.get(&crate::knobs::CAPSULE_FILL)))
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
        .child(activity_ring_on(p, if p.dark { p.layer1 } else { p.bg_base }, 16.0))
        .child(text(label).font(UI).color(p.text));
    if s.activity_open {
        chip = chip.fill(p.pressed);
    }
    Some(row().key("activity-lane").w_full().max_w(crate::view::CONTENT_W).pb(8.0).child(chip))
}

/// The capsule's list (`session-activity-panel`): a floating menu above the capsule, start
/// edges aligned, one 28pt row per running subagent with the spinner, the agent in medium, its
/// task in muted text and the run time in 11pt at the end. Agents and run times line up in
/// columns; the task takes the rest and truncates once the menu reaches 360pt.
pub fn panel(s: &App, p: &Pal, id: &str) -> Option<El> {
    let items = s.running_subagents(id);
    if !s.activity_open || items.is_empty() {
        return None;
    }
    let name_of = |it: &Running| if it.agent.is_empty() { "Agent".to_owned() } else { model::capitalize(&it.agent) };
    let time_of = |it: &Running| if it.started > 0 { elapsed(s.now_ms - it.started) } else { String::new() };
    let width = |t: &str, style| rune::text::measure(t, &[], style, None).0;
    let agent_w = items.iter().map(|it| width(&name_of(it), UI_MED)).fold(0.0, f32::max);
    let time_w = items.iter().map(|it| width(&time_of(it), ELAPSED)).fold(0.0, f32::max);
    let label_w = items.iter().map(|it| width(&it.label, UI)).fold(0.0, f32::max);
    // Panel inset 4 + hairline 0.5, row inset 6, then spinner, agent, task and time, 8 apart,
    // with 8 more before the time.
    let natural = 2.0 * (4.5 + 6.0) + 16.0 + 8.0 + agent_w + 8.0 + label_w + 16.0 + time_w;
    let mut menu = col()
        .key("activity-panel")
        .role("menu")
        .label("Subagents")
        .above("activity-chip", 8.0)
        .w(natural.clamp(240.0, 360.0).ceil())
        .max_h(270.0)
        .padding(4.5)
        .radius(10.0)
        .fill(p.layer1)
        // --v2-elevation-floating: two drops and a hairline ring.
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, p.shadow_far)
        .shadow(4.0, 8.0, p.shadow)
        .transition(rune::Transition::opacity().springs(crate::view::no_motion()))
        .on_press(|_: &mut App| {});
    for it in &items {
        let (parent, child, title, agent) = (id.to_owned(), it.session.clone(), it.label.clone(), it.agent.clone());
        let name = name_of(it);
        menu = menu.child(
            row()
                .key(format!("sub-{}", it.session))
                .role("menuitem")
                .label(format!("{name} {}", it.label))
                .w_full()
                .h(28.0)
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
                .child(activity_ring_on(p, p.layer1, 16.0))
                .child(text(name).font(UI_MED).color(p.text).w(agent_w))
                .child(text(it.label.clone()).font(UI).color(p.muted).truncate().grow(1.0).shrink())
                .child(row().w(time_w).ml(8.0).justify_end().child(text(time_of(it)).font(ELAPSED).color(p.muted))),
        );
    }
    Some(menu)
}

/// The run time in the list: 11pt, tabular figures.
pub(crate) const ELAPSED: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(16.0).features("\"tnum\"");

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use super::*;

    const MAIN: &str = "ses_fx0001aaaaaaaaaaaaaaaaaa";
    const REVIEW: &str = "ses_fx0002aaaaaaaaaaaaaaaaaa";

    fn rt() -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.on_key = Some(crate::on_key);
        rt.frame(0.0);
        rt
    }

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
        // Finished: no spinner, and the call names its child.
        assert!(has(&rt, card, "chip"));
        assert!(!has(&rt, card, "spinner"));
        rt.update(|s, _| {
            s.running.insert(REVIEW.into());
        });
        rt.frame(1.0 / 120.0);
        assert!(has(&rt, card, "spinner"), "the child runs, so the card does");
        assert_eq!(rt.state.tab_status(MAIN), Some(crate::TabStatus::Busy));
        assert!(has(&rt, "", "activity-chip"));

        press(&mut rt, "chip");
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
            vec![Running { session: REVIEW.into(), agent: "general".into(), label: "Review the reconnect change".into(), started: rt.state.session(REVIEW).unwrap().created }]
        );
        press(&mut rt, "activity-chip");
        assert!(has(&rt, "", &format!("sub-{REVIEW}")));
        press(&mut rt, &format!("sub-{REVIEW}"));
        assert_eq!(rt.state.route, Route::Session(REVIEW.into()));
        assert!(!rt.state.activity_open);
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
}
