//! Settings › Preferences › Timeline (`settings/general/general.tsx`'s Timeline section,
//! `settings/timeline-detail.tsx` and `.css`): Timeline detail, a five-stop scale over the
//! presets with what the chosen one does, and Advanced, each activity's visibility, Group and
//! Collapse; Timeline width; Background activity; Changed files; and Wrap code blocks.

use opencode_i18n::t;
use opencode_ui::popup::PopupHost;
use rune::{Color, Drag, El, Key, KeyEvent, Phase, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use opencode_runtime::detail::{Activity, Category, Detail, PRESETS, Placement, preset_description};
use opencode_ui::palette::{Elevate as _, Pal};

use super::choice;
use crate::host::SettingsHost;
use crate::{BackgroundActivity, Choice as _, TimelineWidth, change_settings, list, neutral_lift, set_fold_user, setting, switch};

const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const LINE: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const LINE_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
const STOPS: usize = PRESETS.len();

/// What the section keeps while Settings is open.
#[derive(Default)]
pub struct State {
    /// Advanced, once opened or shut; until then open only for a custom mix.
    advanced: Advanced,
    /// Where each hidden category went back to when shown again (`visiblePlacements`).
    pub shown: [Option<Placement>; 7],
}

#[derive(Default)]
enum Advanced {
    #[default]
    Automatic,
    Open,
    Closed,
}

impl Advanced {
    fn open(&self, custom: bool) -> bool {
        match self {
            Self::Automatic => custom,
            Self::Open => true,
            Self::Closed => false,
        }
    }

    fn toggle(&mut self, custom: bool) {
        *self = if self.open(custom) { Self::Closed } else { Self::Open };
    }
}

fn pick_width(s: &mut dyn SettingsHost, id: &str, _tx: &mut Tx) {
    change_settings(s, |x| x.timeline_width = TimelineWidth::parse(Some(id)));
}

fn pick_background_activity(s: &mut dyn SettingsHost, id: &str, _tx: &mut Tx) {
    change_settings(s, |x| x.background_activity = BackgroundActivity::parse(Some(id)));
}

fn toggle_fold_user(s: &mut dyn SettingsHost) {
    let next = !s.settings().fold_user;
    set_fold_user(s, next);
}

fn toggle_turn_diffs(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.turn_diffs = !x.turn_diffs);
}

fn toggle_wrap_code(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.wrap_code = !x.wrap_code);
}

fn drag_scale(s: &mut dyn SettingsHost, g: &Drag, tx: &mut Tx) {
    if !matches!(g.phase, Phase::Down | Phase::Move) {
        return;
    }
    tx.instant();
    let x = ((g.x - 8.0) / (g.w - 16.0).max(1.0)).clamp(0.0, 1.0);
    let x = if opencode_i18n::direction() == opencode_i18n::Direction::Rtl { 1.0 - x } else { x };
    set_stop(s, (x * (STOPS - 1) as f32).round() as usize);
}

fn key_scale(s: &mut dyn SettingsHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let at = stop(&s.settings().timeline_detail);
    let rtl = opencode_i18n::direction() == opencode_i18n::Direction::Rtl;
    let to = match ev.key {
        Key::Left if rtl => (at + 1).min(STOPS - 1),
        Key::Right if rtl => at.saturating_sub(1),
        Key::Left | Key::Down => at.saturating_sub(1),
        Key::Right | Key::Up => (at + 1).min(STOPS - 1),
        Key::Home => 0,
        Key::End => STOPS - 1,
        _ => return false,
    };
    tx.instant();
    set_stop(s, to);
    true
}

fn toggle_advanced(s: &mut dyn SettingsHost, custom: bool, tx: &mut Tx) {
    tx.instant();
    s.settings_pages_mut().timeline.advanced.toggle(custom);
}

fn toggle_category_visibility(s: &mut dyn SettingsHost, c: Category, tx: &mut Tx) {
    tx.instant();
    let v = s.settings().timeline_detail.get(c);
    let placement = if v.placement == Placement::Hidden {
        s.settings_pages().timeline.shown[c as usize].unwrap_or(Placement::Grouped)
    } else {
        s.settings_pages_mut().timeline.shown[c as usize] = Some(v.placement);
        Placement::Hidden
    };
    change_settings(s, |x| x.timeline_detail.set(c, Activity { placement, ..v }));
}

fn toggle_category_grouped(s: &mut dyn SettingsHost, c: Category) {
    let v = s.settings().timeline_detail.get(c);
    let placement = if v.placement == Placement::Grouped { Placement::Separate } else { Placement::Grouped };
    change_settings(s, |x| x.timeline_detail.set(c, Activity { placement, ..v }));
}

fn toggle_category_collapsed(s: &mut dyn SettingsHost, c: Category) {
    let v = s.settings().timeline_detail.get(c);
    change_settings(s, |x| x.timeline_detail.set(c, Activity { expanded: !v.expanded, ..v }));
}

/// The section: its title over the preference rows.
pub fn section<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let st = s.settings();
    let rows = vec![
        ("settings-timeline-detail", col().key("setting-settings-timeline-detail").w_full().py(20.0).child(detail(s, p))),
        (
            "settings-timeline-width",
            setting(
                p,
                "settings-timeline-width",
                t!("settings.timeline.row.width.title"),
                t!("settings.timeline.row.width.description"),
                choice(s, p, "settings-timeline-width-select", t!("settings.timeline.row.width.title"), st.timeline_width, pick_width),
            ),
        ),
        (
            "settings-background-activity",
            setting(
                p,
                "settings-background-activity",
                t!("settings.timeline.row.backgroundActivity.title"),
                t!("settings.timeline.row.backgroundActivity.description"),
                choice(s, p, "settings-background-activity-select", t!("settings.timeline.row.backgroundActivity.title"), st.background_activity, pick_background_activity),
            ),
        ),
        (
            "settings-fold-user-messages",
            setting(
                p,
                "settings-fold-user-messages",
                t!("settings.timeline.row.foldUserMessages.title"),
                t!("settings.timeline.row.foldUserMessages.description"),
                switch(p, "fold-user-switch", st.fold_user).label_static(t!("settings.timeline.row.foldUserMessages.title")).on_press(|s: &mut H| toggle_fold_user(s)),
            ),
        ),
        (
            "settings-show-turn-diffs",
            setting(
                p,
                "settings-show-turn-diffs",
                t!("settings.timeline.row.turnDiffs.title"),
                t!("settings.timeline.row.turnDiffs.description"),
                switch(p, "turn-diffs-switch", st.turn_diffs).on_press(|s: &mut H| toggle_turn_diffs(s)),
            ),
        ),
        (
            "settings-wrap-code-blocks",
            setting(
                p,
                "settings-wrap-code-blocks",
                t!("settings.timeline.row.wrapCodeBlocks.title"),
                t!("settings.timeline.row.wrapCodeBlocks.description"),
                switch(p, "wrap-code-switch", st.wrap_code).label_static(t!("settings.timeline.row.wrapCodeBlocks.title")).on_press(|s: &mut H| toggle_wrap_code(s)),
            ),
        ),
    ];
    let rows = super::rows(s, p, 20.0, rows);
    super::section(p, "settings-timeline-section", Some(t!("settings.timeline.title")), list(p, "settings-timeline-list", rows))
}

fn stop(d: &Detail) -> usize {
    STOPS - 1 - d.preset().unwrap_or(2)
}

fn detail<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let d = s.settings().timeline_detail;
    let preset = d.preset();
    let (id, name) = preset.map_or(("custom", t!("settings.timeline.custom")), |i| (PRESETS[i].0, opencode_i18n::I18N.text(PRESETS[i].1)));
    let copy = col()
        .w_full()
        .gap(8.0)
        .child(text(t!("settings.timeline.detail")).font(LINE_MED).color(p.text))
        .child(text(t!("settings.timeline.description")).font(BODY).color(p.muted).wrap().my(-3.5));
    let summary = row()
        .key("timeline-detail-summary")
        .w_full()
        .child(text(format!("{} ", t!("settings.timeline.summary", preset = name))).font(BODY).color(p.text))
        .child(text(preset_description(id)).font(BODY).color(p.muted).wrap().shrink().min_w(0.0));
    col()
        .key("timeline-detail-control")
        .w_full()
        .gap(8.0)
        .child(copy)
        .child(scale::<H>(p, preset.map(|i| STOPS - 1 - i)).mt(4.0))
        .child(summary)
        .child(advanced(s, p, &d, preset.is_none()))
}

fn scale<H: SettingsHost + 'static>(p: &Pal, at: Option<usize>) -> El {
    let frac = at.unwrap_or_default() as f32 / (STOPS - 1) as f32;
    let track_bg = if p.dark { p.layer2 } else { p.q.layer4 };
    let marker = opencode_ui::palette::token(p.dark, if p.dark { "grey-700" } else { "grey-500" }).unwrap_or(p.icon_faint);
    let grey300 = opencode_ui::palette::token(p.dark, "grey-300").unwrap_or(p.switch_thumb);
    let accent = p.bg_accent.mix(Color::WHITE, 0.1);
    let dots = (0..STOPS).fold(row().key("stops").pin(Some(-4.0), Some(-3.0), Some(-4.0), None).h(8.0).justify_between(), |r, i| {
        r.child(el().size(8.0, 8.0).radius(4.0).fill(if at.is_some_and(|at| i <= at) { accent } else { marker }))
    });
    let track = el()
        .key("timeline-detail-track")
        .pin(Some(8.0), Some(13.0), Some(8.0), None)
        .h(2.0)
        .radius(1.0)
        .fill(track_bg)
        .child(el().key("progress").pin(Some(0.0), Some(0.0), None, Some(0.0)).w_frac(frac).radius(1.0).fill(accent))
        .child(dots);
    let thumb = at.map(|_| {
        row()
            .key("thumb-lane")
            .pin(Some(0.0), Some(6.0), Some(0.0), None)
            .h(16.0)
            .child(el().grow(frac))
            .child(
                el().key("thumb")
                    .size(16.0, 16.0)
                    .radius(8.0)
                    .center()
                    .gradient(p.bg_accent.mix(Color::WHITE, 0.2), p.bg_accent)
                    .border(0.5, p.q.border_strong)
                    .child(el().size(8.0, 8.0).radius(4.0).fill(grey300).border(0.5, p.q.border_strong)),
            )
            .child(el().grow(1.0 - frac))
    });
    el().key("settings-timeline-detail")
        .role("slider")
        .label_static(t!("settings.timeline.detail"))
        .range_value(at.unwrap_or(2) as f64, 0.0, (STOPS - 1) as f64)
        .range_step(1.0)
        .focusable(true)
        .w_full()
        .h(28.0)
        .on_drag(|s: &mut H, g: &mut Drag, tx: &mut Tx| drag_scale(s, g, tx))
        .on_key(|s: &mut H, ev: &KeyEvent, tx: &mut Tx| key_scale(s, ev, tx))
        .child(row().key("timeline-detail-scale").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).child(track).children(thumb))
}

fn set_stop(s: &mut dyn SettingsHost, at: usize) {
    let d = PRESETS[STOPS - 1 - at.min(STOPS - 1)].2;
    change_settings(s, |x| x.timeline_detail = d);
}

fn advanced<H: SettingsHost + 'static>(s: &H, p: &Pal, d: &Detail, custom: bool) -> El {
    let open = s.settings_pages().timeline.advanced.open(custom);
    let trigger = row()
        .key("timeline-detail-advanced-trigger")
        .role("button")
        .label_static(t!("settings.timeline.advanced"))
        .expanded(open)
        .h(24.0)
        .gap(6.0)
        .center_y()
        .self_start()
        .on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_advanced(s, custom, tx))
        .hover_group()
        .hover_motion(opencode_ui::style::HOVER_FADE, opencode_ui::style::HOVER_FADE)
        .child(text(t!("settings.timeline.advanced")).font(LINE_MED).color(p.text));
    let arrow = el().w(16.0).h(24.0).center().child(opencode_ui::style::arrow(p, open));
    let trigger = trigger.child(arrow);
    let mut c = col().key("timeline-detail-advanced").w_full().pt(8.0).child(trigger);
    if !open {
        return c;
    }
    let columns = row()
        .key("timeline-detail-columns")
        .w_full()
        .h(24.0)
        .mt(-24.0)
        .px(12.5)
        .gap(8.0)
        .center_y()
        .child(el().grow(1.0))
        .child(row().w(72.0).center().child(text(t!("settings.timeline.group")).font(LINE).color(p.muted)))
        .child(row().w(88.0).pl(8.0).center().child(text(t!("settings.timeline.collapse")).font(LINE).color(p.muted)));
    let n = Category::ALL.len();
    let list = Category::ALL.iter().enumerate().fold(
        col().key("timeline-detail-list").w_full().mt(8.0).radius(8.0).fill(p.layer2).border(0.5, p.border).border_layout(),
        |l, (i, c)| l.child(category::<H>(p, d, *c, i + 1 == n)),
    );
    c = c.child(columns).child(list);
    c
}

fn category<H: SettingsHost + 'static>(p: &Pal, d: &Detail, c: Category, last: bool) -> El {
    let v = d.get(c);
    let hidden = v.placement == Placement::Hidden;
    let eye = row()
        .key(format!("timeline-detail-visibility-{}", c.id()))
        .role("button")
        .label(t!("settings.timeline.visibility.label", activity = c.label()))
        .checked(!hidden)
        .help(if hidden { t!("settings.timeline.visibility.show") } else { t!("settings.timeline.visibility.hide") })
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .fill(p.q.neutral)
        .elevate(neutral_lift(p))
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_category_visibility(s, c, tx))
        .child(icon(if hidden { &ic::OUTLINE_EYE_SLASH } else { &ic::OUTLINE_EYE }, 16.0).color(if hidden { p.icon_muted } else { p.icon }));
    let activity = row()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(12.0)
        .center_y()
        .child(eye)
        .child(text(c.label()).font(LINE).color(if hidden { p.faint } else { p.text }).shrink().min_w(0.0));
    let unavailable = || el().w(24.0).h(1.0).fill(p.icon_muted).opacity(0.5);
    let grouped = if hidden {
        unavailable()
    } else {
        switch(p, &format!("timeline-detail-group-{}", c.id()), v.placement == Placement::Grouped)
            .label(t!("settings.timeline.grouped.label", activity = c.label()))
            .on_press(move |s: &mut H| toggle_category_grouped(s, c))
    };
    let collapse = c.has_details().then(|| {
        if hidden {
            return unavailable();
        }
        switch(p, &format!("timeline-detail-collapse-{}", c.id()), !v.expanded)
            .label(t!("settings.timeline.collapsed.label", activity = c.label()))
            .on_press(move |s: &mut H| toggle_category_collapsed(s, c))
    });
    let r = row()
        .key(format!("timeline-detail-category-{}", c.id()))
        .w_full()
        .h(40.0)
        .pl(9.5)
        .pr(12.0)
        .gap(8.0)
        .center_y()
        .child(activity)
        .child(row().w(72.0).center().child(grouped))
        .child(row().w(88.0).pl(8.0).center().children(collapse));
    if last { r } else { r.border_sides([0.0, 0.0, 0.5, 0.0], p.border).border_layout() }
}
