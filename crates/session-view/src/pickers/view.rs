//! The pickers on screen: the toolbar's triggers, the open menu, and the layer a host mounts
//! over its window ([`layer`]). The model picker is a dialog of its own ([`super::dialog`]).

use opencode_kit::model::capitalize;
use rune::{Align, Color, El, Side, Spring, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{ADD_PROJECT_KEY, Kind, PickerHost};
use crate::palette::Pal;
use crate::popover::{self as pop, MENU, Opt};
use crate::style::UI;

/// The open picker over everything: the model dialog, or a menu over a scrim that closes it.
pub fn layer<S: PickerHost>(s: &S, p: &Pal) -> Vec<El> {
    // A narrow window picks from drawers (`composer.tsx:192`, `editor.tsx:1126`); a project
    // keeps its menu.
    if crate::style::narrow() && s.pickers().open.is_some_and(|k| k != Kind::Project) {
        let win_h = rune::memo::layout_with("zoom", |r| r[3]).map_or(800.0, |h| h / s.zoom());
        return super::drawer::layer(s, p, win_h);
    }
    match s.pickers().open {
        None => super::manage::layer(s, p),
        Some(Kind::Model) => super::dialog::layer(s, p),
        // Nothing gets focus back, so the search field can unmount in this frame.
        Some(_) => vec![pop::scrim::<S>("picker-dismiss", |s: &mut S, _| s.close_picker(false)), picker(s, p)],
    }
}

/// A toolbar button that opens the `kind` list, labelled with what's picked.
pub fn trigger<S: PickerHost>(s: &S, p: &Pal, kind: Kind, label: &str, color: Color, chevron: bool) -> El {
    let open = s.pickers().open == Some(kind);
    let label = text(label.to_owned()).font(UI).color(if open { p.text } else { color }).truncate_fade();
    wrap(s, p, kind, crate::composer::ghost_with(p, kind.key(), label, chevron, open))
}

/// The model's button, as Desktop's chip (`composer.tsx:158-176`): its provider in
/// `text-faint`, a slash, then the model, unless the model needs no provider (OpenCode's own).
/// It opens a dialog rather than a menu, so it never shows as pressed (`composer.tsx:106-111`).
pub fn model_trigger<S: PickerHost>(s: &S, p: &Pal, provider: Option<&str>, model: &str, chevron: bool) -> El {
    let name = text(model.to_owned()).key("model-name").font(UI).color(p.muted).shrink().truncate_fade();
    let label = row().key("model-label").shrink().min_w(0.0).center_y().children(provider.map(|provider| {
        text(provider.to_owned()).key("model-provider").font(UI).color(p.faint).shrink().truncate_fade()
    }));
    let label = if provider.is_some() { label.child(crate::composer::slash(p, 6.0)) } else { label };
    wrap(s, p, Kind::Model, crate::composer::ghost_with(p, Kind::Model.key(), label.child(name), chevron, false))
}

/// A trigger's states: pressed while its list is open (the
/// ghost's `[data-expanded]`, `button.css:259-278`, which drops the hover), and opening its
/// list on a press.
fn wrap<S: PickerHost>(s: &S, p: &Pal, kind: Kind, button: El) -> El {
    // A new label resizes it and moves its neighbours in the choice, with no glide.
    let mut button = button.label(kind.choose_label()).motion(Spring::NONE);
    if s.pickers().open == Some(kind) && kind != Kind::Model {
        button = button.fill(p.pressed).hover(Color::TRANSPARENT);
    }
    button.on_press_tx(move |s: &mut S, tx: &mut Tx| open::<S>(s, kind, tx))
}

/// The agent's picker folded to its mark; it still opens the agent list.
pub fn agent_icon<S: PickerHost>(_: &S, p: &Pal, name: &str) -> El {
    let b = el()
        .key(Kind::Agent.key())
        .role("button")
        .label(opencode_i18n::t!("ui.promptInput.chooseAgent"))
        .help(format!("{} ({name})", opencode_i18n::t!("ui.promptInput.chooseAgent")))
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .motion(Spring::NONE)
        .focus_ring(2.0, -2.0, p.v2.border_focus)
        .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted));
    b.on_press_tx(|s: &mut S, tx: &mut Tx| open::<S>(s, Kind::Agent, tx))
}

fn open<S: PickerHost>(s: &mut S, kind: Kind, tx: &mut Tx) {
    s.open_picker(kind);
    match s.pickers().open {
        // Desktop's dialog is whole in the frame it opens (`context/dialog.tsx`).
        Some(Kind::Model) => tx.instant(),
        Some(_) => pop::motion::opening(s.knobs(), tx),
        None => {}
    }
}

/// The agent, model and effort triggers for a toolbar with room for all of them: the agent's
/// only when the host shows it, the effort's only for a model that has efforts.
pub fn triggers<S: PickerHost>(s: &S, p: &Pal) -> Vec<El> {
    let selected = s.selection();
    let model = selected.model;
    let mut out = Vec::new();
    if s.picker_show_agent() {
        out.push(trigger(s, p, Kind::Agent, &capitalize(&selected.agent), p.muted, true));
    }
    let name = s.pickers().model_name(&model);
    let provider = s.pickers().provider_label(&model);
    out.push(model_trigger(s, p, provider.as_deref(), &name, true).shrink().min_w(0.0));
    if s.has_effort() {
        out.push(effort_trigger(s, p, &model.variant, true));
    }
    out
}

/// The effort's button. "Default", the absence of a choice, shows only while the pointer is
/// over the composer, the keyboard is on it or its list is open (`editor.css:75-80`); it keeps
/// its room meanwhile.
pub fn effort_trigger<S: PickerHost>(s: &S, p: &Pal, variant: &str, chevron: bool) -> El {
    let default = variant.is_empty() || variant == "default";
    let label = if default { opencode_i18n::t!("common.default").to_owned() } else { capitalize(variant) };
    let button = trigger(s, p, Kind::Effort, &label, p.muted, chevron);
    // Rune shows it at its hovered opacity while the keyboard is on it (`:focus-visible`).
    let shown = !default || s.pickers().open == Some(Kind::Effort);
    if shown { button } else { button.group_opacity(0.0, 1.0) }
}

/// The open menu (agent, effort or project), above its trigger.
pub fn picker<S: PickerHost>(s: &S, p: &Pal) -> El {
    let kind = s.pickers().open.unwrap();
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = pop::surface::<S>(p, MENU, format!("picker-{}", kind.key()), kind.choose_label()).w(if kind == Kind::Project { 240.0 } else { 180.0 });
    let mut list = pop::List::new("picker-options", MENU);
    let mut footer: Option<El> = None;
    let mut group = "";
    for item in &items {
        let is_add = kind == Kind::Project && item.key == ADD_PROJECT_KEY;
        if !is_add && !item.group.is_empty() && item.group != group {
            let heading = if item.heading.is_empty() { item.group.as_str() } else { item.heading.as_str() };
            list.header(p, format!("group-{}", item.group), heading);
            group = &item.group;
        }
        let key = item.key.clone();
        let is_active = active.as_deref() == Some(key.as_str());
        let mut o = Opt::new(format!("option-{key}"), item.label.clone()).active(is_active).checked(!is_add && item.key == selected);
        if is_add {
            o = o.role("menuitem").lead(icon(&ic::PLUS, 16.0).color(p.icon_muted)).trailing(pop::hint(p, &crate::keymap::Bind::OpenProject.label()));
        } else {
            o = o.slides();
            if kind == Kind::Project && let Some(lead) = s.picker_project_lead(p, &item.key, &item.label) {
                o = o.lead(lead);
            }
        }
        let hover_key = key.clone();
        let row = pop::option(
            p,
            MENU,
            o,
            move |s: &mut S| s.pickers_mut().activate(hover_key.clone()),
            move |s: &mut S, tx: &mut Tx| s.choose_picker_tx(&key, tx),
        );
        if is_add {
            footer = Some(row);
        } else {
            list.option(row, is_active);
        }
    }
    list.slide(s.knobs(), p, &s.pickers().search.text);
    let listed = !list.is_empty();
    if listed {
        menu = menu.child(list.build("picker-scroll", 240.0));
    }
    if let Some(add_row) = footer {
        if listed {
            menu = menu.child(pop::divider(p, "picker-add-divider"));
        }
        menu = menu.child(col().key("picker-footer").w_full().padding(MENU.inset).child(add_row));
    }
    pop::place(s.knobs(), menu, kind.key(), Side::Top, Align::Start, 6.0)
}
