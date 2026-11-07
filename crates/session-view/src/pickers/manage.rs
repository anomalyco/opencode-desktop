//! Manage models, the model list's sliders button (Desktop's `DialogManageModels`,
//! `providers/models/manage.tsx`, `manage.css`): every model the server lists, by provider, each
//! with a switch that shows or hides it in the list, and one per provider for all of its models.
//! The choices are the prefs' `user` visibility (`models.tsx:116-144`).

use std::collections::HashSet;

use rune::{Color, El, Key, ScrollSpec, Shadow, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, spacer, text, text_input};
use rune_theme_opencode::icons as ic;

use super::{Model, PickerHost, host, rail, search};
use crate::palette::Pal;
use crate::style::{UI, UI_MED};

/// The open dialog: its query and the providers folded shut.
pub struct Manage {
    pub search: TextEdit,
    pub collapsed: HashSet<String>,
}

/// `dialog-title` and `dialog-description` as the settings dialog sets them: 13 on 20 at 440
/// in `text-base`, 12 on 20 muted.
const TITLE: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);
const DESCRIPTION: TextStyle = TextStyle::new(12.0, 440.0).lh(20.0).tracking(-0.04);

impl super::Pickers {
    /// Opens Manage models with an empty search, its field taking the keys.
    pub fn open_manage(&mut self) {
        let mut search = TextEdit::new("");
        search.focused = true;
        self.manage = Some(Manage { search, collapsed: HashSet::new() });
    }

    pub fn close_manage(&mut self) {
        self.manage = None;
    }

    /// Every model the query finds by provider, name or id (`manage.tsx:48-68`), each provider's
    /// together by name, the providers in Desktop's order.
    fn manage_groups(&self) -> Vec<(String, Vec<&Model>)> {
        let query = self.manage.as_ref().map_or("", |m| m.search.text.as_str()).trim().to_owned();
        let mut models: Vec<&Model> = self.models.iter()
            .filter(|m| query.is_empty() || search::score(&query, &[m.group.as_str(), m.name.as_str(), m.id.as_str()]).is_some())
            .collect();
        models.sort_by(|a, b| host::provider_rank(a).cmp(&host::provider_rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        let mut groups: Vec<(String, Vec<&Model>)> = Vec::new();
        for m in models {
            match groups.last_mut() {
                Some((p, list)) if *p == m.provider => list.push(m),
                _ => groups.push((m.provider.clone(), vec![m])),
            }
        }
        groups
    }

    fn set_shown(&mut self, keys: Vec<String>, shown: bool) {
        self.prefs.set_visibility(keys.iter().map(String::as_str), shown);
    }
}

/// The scrim and the dialog over the window while Manage models is open.
pub fn layer<S: PickerHost>(s: &S, p: &Pal) -> Vec<El> {
    let pk = s.pickers();
    let Some(m) = &pk.manage else { return Vec::new() };
    let groups = pk.manage_groups();
    let searching = !m.search.text.trim().is_empty();

    let field = text_input(&m.search, UI)
        .key("manage-models-input")
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder("Search", p.faint)
        .caret(p.text, p.selection)
        .on_edit(|s: &mut S| &mut s.pickers_mut().manage.as_mut().expect("open").search)
        .on_input_key(|s: &mut S, ev, tx: &mut Tx| {
            if ev.key != Key::Escape || s.pickers().manage.as_ref().is_some_and(|m| m.search.preedit.is_some()) {
                return false;
            }
            tx.instant();
            s.pickers_mut().close_manage();
            true
        });
    // `manage-models-search`: 6 pt in, a 28 pt field with its glass 8 pt in and the query 8 pt
    // after it; a hairline across the dialog under it.
    let search_row = col().key("manage-models-search").w_full().px(6.0)
        .child(row().key("field").h(28.0).w_full().pl(8.0).pr(8.0).gap(8.0).center_y()
            .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
            .child(field));
    let hairline = el().key("manage-models-rule").w_full().h(0.5).mt(6.0).fill(p.border_muted);

    let mut body = col().key("manage-models-list").w_full().px(8.0).pt(8.0).pb(8.0);
    if groups.is_empty() {
        let mut status = row().key("manage-models-empty").w_full().min_h(120.0).gap(4.0).center()
            .child(text("No model results").font(UI).color(p.muted));
        if searching {
            status = status.child(text(format!("\"{}\"", m.search.text.trim())).font(UI).color(p.faint));
        }
        body = body.child(status);
    }
    for (at, (provider, models)) in groups.iter().enumerate() {
        let open = searching || !m.collapsed.contains(provider);
        // `.settings-section + .settings-section`: a 0.5 pt rule above each after the first.
        if at > 0 {
            body = body.child(el().key(format!("manage-rule-{provider}")).w_full().h(0.5).fill(p.border));
        }
        let mut section = col().key(format!("manage-provider-{provider}")).w_full().py(8.0).gap(4.0);
        section = section.child(header::<S>(s, p, provider, models, open, searching));
        if open {
            section = section.child(rows::<S>(s, p, models));
        }
        body = body.child(section);
    }

    let dialog = overlay_shadow(p).into_iter().fold(col(), El::box_shadow)
        .key("manage-models")
        .role("dialog")
        .label("Manage models")
        .w(640.0)
        .h(480.0)
        .radius(12.0)
        .fill(p.bg_base)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(rune::Spring::NONE)
        .modal(true, |s: &mut S| s.pickers_mut().close_manage())
        // `dialog-header`: 10 pt above, 14 before, 6 below the title and its description.
        .child(col().key("manage-models-header").w_full().pt(10.0).pr(10.0).pb(6.0).pl(14.0).gap(2.0)
            .child(text("Manage models").font(TITLE).color(p.text))
            .child(text("Customize which models appear in the model selector.").font(DESCRIPTION).color(p.muted)))
        .child(search_row)
        .child(hairline)
        .child(col().key("manage-models-scroll").w_full().grow(1.0).shrink().min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(body));
    let scrim = el().key("manage-models-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    let modal = el().key("manage-models-modal").absolute(0.0, 0.0).w_full().h_full().center().transition(Transition::none()).child(dialog).raise();
    vec![scrim, modal]
}

/// A provider's header (`provider-group.tsx:98-120`, `manage.css:34-60`): its chevron, mark and
/// name fold it, and its switch shows or hides every model it has, 24 pt in from the end.
fn header<S: PickerHost>(s: &S, p: &Pal, provider: &str, models: &[&Model], open: bool, searching: bool) -> El {
    let pk = s.pickers();
    let all = models.iter().all(|m| pk.visible(m));
    let name = models.first().map_or(provider.to_owned(), |m| m.group.clone());
    let first = models.first().map_or(String::new(), |m| m.key.clone());
    let fold = provider.to_owned();
    let mut trigger = row().key("trigger").role("button").label(format!("{name} models")).h(28.0)
        .pt(4.0).pr(8.0).pb(4.0).pl(4.0).gap(4.0).center_y().radius(4.0)
        .child(row().key("chevron").size(20.0, 20.0).center()
            .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })))
        .child(row().key("label").gap(8.0).center_y()
            .child(icon(rail::mark(provider, &first), 16.0).color(p.text))
            .child(text(name.clone()).font(UI_MED).color(p.text)));
    if !searching {
        trigger = trigger.hover(p.hover).press(p.pressed).on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            let Some(m) = s.pickers_mut().manage.as_mut() else { return };
            if !m.collapsed.remove(&fold) {
                m.collapsed.insert(fold.clone());
            }
        });
    }
    let keys: Vec<String> = models.iter().map(|m| m.key.clone()).collect();
    let flip = switch::<S>(p, &format!("switch-{provider}"), all, move |s: &mut S| s.pickers_mut().set_shown(keys.clone(), !all));
    row().key("header").w_full().h(28.0).center_y().child(trigger).child(spacer()).child(flip.mr(24.0))
}

/// A provider's models (`manage.tsx:74-99`, `manage.css:81-113`): 36 pt rows 1 pt apart, a
/// hairline between them, each a press that flips its switch.
fn rows<S: PickerHost>(s: &S, p: &Pal, models: &[&Model]) -> El {
    let pk = s.pickers();
    let mut list = col().key("rows").w_full();
    for (at, m) in models.iter().enumerate() {
        let shown = pk.visible(m);
        let key = m.key.clone();
        let press = key.clone();
        let r = row().key(format!("manage-{key}")).role("switch").label(m.name.clone()).w_full().h(36.0).px(8.0).gap(12.0).center_y().radius(6.0)
            .hover(p.hover).press(p.pressed)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.instant();
                s.pickers_mut().set_shown(vec![press.clone()], !shown);
            })
            .child(text(m.name.clone()).font(UI_MED).color(p.text).grow(1.0).shrink().min_w(0.0).truncate())
            .child(switch::<S>(p, "switch", shown, move |s: &mut S| s.pickers_mut().set_shown(vec![key.clone()], !shown)));
        list = list.child(r);
        // The 1 pt gap, its hairline across the middle and 8 pt in from each end.
        if at + 1 < models.len() {
            list = list.child(col().key(format!("rule-{at}")).w_full().h(1.0).px(8.0).pt(0.25).child(el().w_full().h(0.5).fill(p.border)));
        }
    }
    list
}

/// Desktop's switch (`switch.css`): 24 × 16 with 4 pt corners, a 12 pt thumb with 2 pt corners
/// that sits at the end when on, the accent under it then. It flips in the press's frame.
fn switch<S: PickerHost>(p: &Pal, key: &str, on: bool, flip: impl Fn(&mut S) + 'static) -> El {
    let thumb = el().key("thumb").size(12.0, 12.0).radius(2.0).fill(p.switch_thumb);
    row().key(key.to_owned()).role("switch").label(if on { "On" } else { "Off" })
        .w(24.0).h(16.0).px(2.0).center_y().radius(4.0)
        .fill(if on { p.bg_accent } else { p.q.layer4 })
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            flip(s);
        })
        .child(if on { row().w_full().child(spacer()).child(thumb) } else { row().w_full().child(thumb) })
}

/// `--v2-elevation-overlay` (`theme.css:112-114`, `:230-234`): two drops and a 0.5 pt ring.
fn overlay_shadow(p: &Pal) -> [Shadow; 3] {
    let drop = |y, blur, spread, color| Shadow { y, blur, spread, color, ..Shadow::default() };
    let (dark, light) = (Color::BLACK, Color::WHITE);
    if p.dark {
        [drop(16.0, 32.0, 0.0, dark.alpha(0.3)), drop(8.0, 16.0, 0.0, dark.alpha(0.3)), drop(0.0, 0.0, 0.5, light.alpha(0.16))]
    } else {
        [drop(16.0, 32.0, 0.0, dark.alpha(0.04)), drop(8.0, 16.0, 0.0, dark.alpha(0.08)), drop(0.0, 0.0, 0.5, dark.alpha(0.12))]
    }
}
