//! Settings › Preferences › Timeline (`settings/general/general.tsx`'s Timeline section,
//! `settings/timeline-detail.tsx` and `.css`): Timeline detail, a five-stop scale over the
//! presets with what the chosen one does, and Advanced, each activity's visibility, Group and
//! Collapse; Timeline width; Background activity; and Changed files.

use rune::{Color, Drag, El, Key, Phase, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use session_view::detail::{Activity, Category, Detail, PRESETS, Placement, preset_description};

use super::choice;
use crate::App;
use crate::palette::{Elevate as _, Pal};
use crate::settings::{self, BackgroundActivity, Choice as _, TimelineWidth, setting, switch};
use opencode_i18n::t;

/// The control's text: 13 on 20 at 440, -0.04 tracking (`timeline-detail-control`).
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
/// Its compact lines (the table, Advanced): 13 on 16.
const LINE: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const LINE_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// The slider's stops, quietest first (`timelinePresets.toReversed()`).
const STOPS: usize = PRESETS.len();

/// What the section keeps while Settings is open.
#[derive(Default)]
pub struct State {
    /// Advanced, once opened or shut; until then open only for a custom mix.
    pub advanced: Option<bool>,
    /// Where each hidden category went back to when shown again (`visiblePlacements`).
    pub shown: [Option<Placement>; 7],
}

/// The section: its title over its card of four rows.
pub fn section(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let rows = vec![
        ("settings-timeline-detail", col().key("setting-settings-timeline-detail").w_full().py(20.0).child(detail(s, p))),
        ("settings-timeline-width", setting(p, "settings-timeline-width", t!("settings.timeline.row.width.title"), t!("settings.timeline.row.width.description"),
            choice(s, p, "settings-timeline-width-select", t!("settings.timeline.row.width.title"), st.timeline_width, |s, id, _| s.change_settings(|x| x.timeline_width = TimelineWidth::parse(Some(id)))))),
        ("settings-background-activity", setting(p, "settings-background-activity", t!("settings.timeline.row.backgroundActivity.title"), t!("settings.timeline.row.backgroundActivity.description"),
            choice(s, p, "settings-background-activity-select", t!("settings.timeline.row.backgroundActivity.title"), st.background_activity, |s, id, _| s.change_settings(|x| x.background_activity = BackgroundActivity::parse(Some(id)))))),
        ("settings-show-turn-diffs", setting(p, "settings-show-turn-diffs", t!("settings.timeline.row.turnDiffs.title"), t!("settings.timeline.row.turnDiffs.description"),
            switch(p, "turn-diffs-switch", st.turn_diffs).on_press(|s: &mut App| s.change_settings(|x| x.turn_diffs = !x.turn_diffs)))),
    ];
    let rows = super::rows(s, p, 20.0, rows);
    super::section(p, "settings-timeline-section", Some(t!("settings.timeline.title")), settings::list(p, "settings-timeline-list", rows))
}

/// The slider's stop for `d`: its preset's, or Compact's for a custom mix.
fn stop(d: &Detail) -> usize {
    STOPS - 1 - d.preset().unwrap_or(2)
}

/// `TimelineDetailControl`.
fn detail(s: &App, p: &Pal) -> El {
    let d = s.settings.timeline_detail;
    let preset = d.preset();
    let (id, name) = preset.map_or(("custom", t!("settings.timeline.custom")), |i| (PRESETS[i].0, opencode_i18n::I18N.text(PRESETS[i].1)));
    let copy = col().w_full().gap(8.0)
        .child(text(t!("settings.timeline.detail")).font(LINE_MED).color(p.text))
        .child(text(t!("settings.timeline.description")).font(BODY).color(p.muted).wrap().my(-3.5));
    let summary = row().key("timeline-detail-summary").w_full()
        .child(text(format!("{} ", t!("settings.timeline.summary", preset = name))).font(BODY).color(p.text))
        .child(text(preset_description(id)).font(BODY).color(p.muted).wrap().shrink().min_w(0.0));
    col().key("timeline-detail-control").w_full().gap(8.0)
        .child(copy)
        .child(scale(p, stop(&d)).mt(16.0))
        .child(summary)
        .child(advanced(s, p, &d, preset.is_none()))
}

/// The five-stop scale: a 4 pt track filled to the stop in the accent, its stops 6 pt dots
/// ringed in the card's fill, and a 16 pt thumb (`timeline-detail-scale`, the range input).
/// A press or drag anywhere on it picks the nearest stop; the arrows step it.
fn scale(p: &Pal, at: usize) -> El {
    let frac = at as f32 / (STOPS - 1) as f32;
    let track_bg = if p.dark { p.layer3 } else { p.q.layer4 };
    let marker = session_view::palette::token(p.dark, "grey-500").unwrap_or(p.icon_faint);
    let grey300 = session_view::palette::token(p.dark, "grey-300").unwrap_or(p.switch_thumb);
    let depth = |name: &str| session_view::palette::token(p.dark, &format!("overlay-gradient-depth-overlay-depth-{name}")).unwrap_or(Color::TRANSPARENT);
    // Grey-300 under the depth gradient, as the thumb (and a dark stop) paints.
    let raised = |size: f32| el().size(size, size).radius(size / 2.0).fill(grey300)
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(size / 2.0).gradient(depth("top"), depth("bot")));
    let dots = (0..STOPS).fold(row().key("stops").pin(Some(-5.0), Some(-3.0), Some(-5.0), None).h(10.0).justify_between(), |r, i| {
        let ring = |inner: El| el().size(10.0, 10.0).radius(5.0).fill(p.layer1).center().child(inner);
        r.child(match (i <= at, p.dark) {
            (true, _) => ring(el().size(6.0, 6.0).radius(3.0).fill(p.bg_accent)),
            (false, false) => ring(el().size(6.0, 6.0).radius(3.0).fill(marker)),
            (false, true) => ring(raised(6.0)),
        })
    });
    let track = el().key("timeline-detail-track").pin(Some(8.0), Some(12.0), Some(8.0), None).h(4.0).radius(2.0).fill(track_bg)
        .child(el().key("progress").pin(Some(0.0), Some(0.0), None, Some(0.0)).w_frac(frac).radius(2.0).fill(p.bg_accent))
        .child(dots);
    // The range's thumb travels its width less its own, so its centre lands on each stop.
    let thumb = row().key("thumb-lane").pin(Some(0.0), Some(6.0), Some(0.0), None).h(16.0)
        .child(el().grow(frac))
        .child(raised(16.0).key("thumb").elevate(crate::status_toast::neutral_lift(p)))
        .child(el().grow(1.0 - frac));
    let pick = |s: &mut App, g: &mut Drag, tx: &mut Tx| {
        if !matches!(g.phase, Phase::Down | Phase::Move) {
            return;
        }
        tx.instant();
        let x = ((g.x - 8.0) / (g.w - 16.0).max(1.0)).clamp(0.0, 1.0);
        set_stop(s, (x * (STOPS - 1) as f32).round() as usize);
    };
    el().key("settings-timeline-detail").role("slider").label(t!("settings.timeline.detail")).range_value(at as f64, 0.0, (STOPS - 1) as f64).range_step(1.0).focusable(true).w_full().h(28.0)
        .on_drag(pick)
        .on_key(|s: &mut App, ev, tx| {
            let at = stop(&s.settings.timeline_detail);
            let to = match ev.key {
                Key::Left | Key::Down => at.saturating_sub(1),
                Key::Right | Key::Up => (at + 1).min(STOPS - 1),
                Key::Home => 0,
                Key::End => STOPS - 1,
                _ => return false,
            };
            tx.instant();
            set_stop(s, to);
            true
        })
        .child(row().key("timeline-detail-scale").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).child(track).child(thumb))
}

fn set_stop(s: &mut App, at: usize) {
    let d = PRESETS[STOPS - 1 - at.min(STOPS - 1)].2;
    s.change_settings(|x| x.timeline_detail = d);
}

/// Advanced (`timeline-detail-advanced`): its trigger, and while open each activity's row.
fn advanced(s: &App, p: &Pal, d: &Detail, custom: bool) -> El {
    let open = s.settings_pages.timeline.advanced.unwrap_or(custom);
    let trigger = row().key("timeline-detail-advanced-trigger").role("button").label(t!("settings.timeline.advanced")).expanded(open).h(24.0).gap(6.0).center_y().self_start()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.settings_pages.timeline.advanced = Some(!open);
        })
        .hover_group()
        .hover_motion(session_view::style::HOVER_FADE, session_view::style::HOVER_FADE)
        .child(text(t!("settings.timeline.advanced")).font(LINE_MED).color(p.text));
    // The arrow shows under the pointer, and always while open (`collapsible.css`).
    let arrow = el().w(16.0).h(24.0).center().child(session_view::style::arrow(p, open));
    let trigger = trigger.child(if open { arrow } else { arrow.group_opacity(0.0, 1.0) });
    let mut c = col().key("timeline-detail-advanced").w_full().pt(8.0).child(trigger);
    if !open {
        return c;
    }
    // The column names share the trigger's line, the list 8 under them.
    let columns = row().key("timeline-detail-columns").w_full().h(24.0).mt(-24.0).px(12.5).gap(8.0).center_y()
        .child(el().grow(1.0))
        .child(row().w(72.0).center().child(text(t!("settings.timeline.group")).font(LINE).color(p.muted)))
        .child(row().w(88.0).pl(8.0).center().child(text(t!("settings.timeline.collapse")).font(LINE).color(p.muted)));
    let n = Category::ALL.len();
    let list = Category::ALL.iter().enumerate().fold(
        col().key("timeline-detail-list").w_full().mt(8.0).radius(8.0).fill(p.layer2).border(0.5, p.border).border_layout(),
        |l, (i, c)| l.child(category(p, d, *c, i + 1 == n)),
    );
    c = c.child(columns).child(list);
    c
}

/// One activity's row: its visibility and name, Group, and Collapse for shells, edits and thinking.
fn category(p: &Pal, d: &Detail, c: Category, last: bool) -> El {
    let v = d.get(c);
    let hidden = v.placement == Placement::Hidden;
    let eye = row().key(format!("timeline-detail-visibility-{}", c.id())).role("button").label(t!("settings.timeline.visibility.label", activity = c.label())).expanded(!hidden)
        .help(if hidden { t!("settings.timeline.visibility.show") } else { t!("settings.timeline.visibility.hide") })
        .size(20.0, 20.0).center().radius(4.0).hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let v = s.settings.timeline_detail.get(c);
            let placement = if v.placement == Placement::Hidden {
                s.settings_pages.timeline.shown[c as usize].unwrap_or(Placement::Grouped)
            } else {
                s.settings_pages.timeline.shown[c as usize] = Some(v.placement);
                Placement::Hidden
            };
            s.change_settings(|x| x.timeline_detail.set(c, Activity { placement, ..v }));
        })
        .child(icon(if hidden { &ic::OUTLINE_EYE_SLASH } else { &ic::OUTLINE_EYE }, 16.0).color(if hidden { p.icon_muted } else { p.icon }));
    let activity = row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y()
        .child(eye)
        .child(text(c.label()).font(LINE).color(if hidden { p.faint } else { p.text }).shrink().min_w(0.0));
    let unavailable = || el().w(24.0).h(1.0).fill(p.icon_muted).opacity(0.5);
    let grouped = if hidden {
        unavailable()
    } else {
        switch(p, &format!("timeline-detail-group-{}", c.id()), v.placement == Placement::Grouped).label(t!("settings.timeline.grouped.label", activity = c.label()))
            .on_press(move |s: &mut App| {
                let v = s.settings.timeline_detail.get(c);
                let placement = if v.placement == Placement::Grouped { Placement::Separate } else { Placement::Grouped };
                s.change_settings(|x| x.timeline_detail.set(c, Activity { placement, ..v }));
            })
    };
    let collapse = c.has_details().then(|| {
        if hidden {
            return unavailable();
        }
        switch(p, &format!("timeline-detail-collapse-{}", c.id()), !v.expanded).label(t!("settings.timeline.collapsed.label", activity = c.label()))
            .on_press(move |s: &mut App| {
                let v = s.settings.timeline_detail.get(c);
                s.change_settings(|x| x.timeline_detail.set(c, Activity { expanded: !v.expanded, ..v }));
            })
    });
    let r = row().key(format!("timeline-detail-category-{}", c.id())).w_full().h(40.0).px(12.0).gap(8.0).center_y()
        .child(activity)
        .child(row().w(72.0).center().child(grouped))
        .child(row().w(88.0).pl(8.0).center().children(collapse));
    if last { r } else { r.border_sides([0.0, 0.0, 0.5, 0.0], p.border).border_layout() }
}
