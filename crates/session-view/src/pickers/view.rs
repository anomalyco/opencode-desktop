//! The pickers on screen: the toolbar's triggers, the open list, and the layer a host mounts
//! over its window ([`layer`]).

use opencode_kit::model::capitalize;
use rune::{Align, Color, El, Side, Spring, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::flyout::{self, EffortFlyout};
use super::hover::{self, ModelHover};
use super::{ADD_PROJECT_KEY, Kind, ModelAction, PickerHost, rail};
use crate::palette::Pal;
use crate::popover::{self as pop, LIST, MENU, Opt};
use crate::style::UI;

/// The scrim, the open list and the lit model's efforts, over everything, while a picker is open.
pub fn layer<S: PickerHost>(s: &S, p: &Pal) -> Vec<El> {
    if s.pickers().open.is_none() {
        return super::manage::layer(s, p);
    }
    // Nothing gets focus back, so the search field can unmount in this frame.
    let scrim = pop::scrim::<S>("picker-dismiss", |s: &mut S, _| s.close_picker(false));
    let mut out = vec![hover::watch::<S>(scrim), picker(s, p)];
    out.extend(flyout::view(s, p));
    out
}

/// A toolbar button that opens the `kind` list, labelled with what's picked.
pub fn trigger<S: PickerHost>(s: &S, p: &Pal, kind: Kind, label: &str, color: Color, chevron: bool) -> El {
    let open = s.pickers().open == Some(kind);
    let label = text(label.to_owned()).font(UI).color(if open { p.text } else { color }).truncate_fade();
    wrap(s, p, kind, crate::composer::ghost_with(p, kind.key(), label, chevron, open))
}

/// The model's button, as Desktop's chip (`composer.tsx:158-176`): its provider in
/// `text-faint`, a slash, then the model, unless the model needs no provider (OpenCode's own).
pub fn model_trigger<S: PickerHost>(s: &S, p: &Pal, provider: Option<&str>, model: &str, chevron: bool) -> El {
    let open = s.pickers().open == Some(Kind::Model);
    let name = text(model.to_owned()).key("model-name").font(UI).color(if open { p.text } else { p.muted }).shrink().truncate_fade();
    let label = row().key("model-label").shrink().min_w(0.0).center_y().children(provider.map(|provider| {
        text(provider.to_owned()).key("model-provider").font(UI).color(p.faint).shrink().truncate_fade()
    }));
    let label = if provider.is_some() { label.child(crate::composer::slash(p, 6.0)) } else { label };
    wrap(s, p, Kind::Model, crate::composer::ghost_with(p, Kind::Model.key(), label.child(name), chevron, open))
}

/// A trigger's states: lit while the keyboard is on it, pressed while its list is open (the
/// ghost's `[data-expanded]`, `button.css:259-278`, which drops the hover), and opening its
/// list on a press.
fn wrap<S: PickerHost>(s: &S, p: &Pal, kind: Kind, button: El) -> El {
    // A new label resizes it and moves its neighbours in the choice, with no glide.
    let mut button = button.label(format!("Choose {}", kind.key())).motion(Spring::NONE);
    if s.pickers().open == Some(kind) {
        button = button.fill(p.pressed).hover(Color::TRANSPARENT);
    } else if !s.composer_edit().focused && s.pickers().toolbar_focus == Some(kind) {
        button = button.fill(p.hover);
    }
    button.on_press_tx(move |s: &mut S, tx: &mut Tx| open::<S>(s, kind, tx))
}

/// The agent's picker folded to its mark; it still opens the agent list.
pub fn agent_icon<S: PickerHost>(s: &S, p: &Pal, name: &str) -> El {
    let mut b = el()
        .key(Kind::Agent.key())
        .role("button")
        .label("Choose agent")
        .help(format!("Choose agent ({name})"))
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .motion(Spring::NONE)
        .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted));
    if !s.composer_edit().focused && s.pickers().toolbar_focus == Some(Kind::Agent) {
        b = b.fill(p.hover);
    }
    b.on_press_tx(|s: &mut S, tx: &mut Tx| open::<S>(s, Kind::Agent, tx))
}

fn open<S: PickerHost>(s: &mut S, kind: Kind, tx: &mut Tx) {
    s.open_picker(kind);
    if s.pickers().open.is_some() {
        pop::motion::opening(s.knobs(), tx);
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
    let label = if default { "Default".to_owned() } else { capitalize(variant) };
    let button = trigger(s, p, Kind::Effort, &label, p.muted, chevron);
    let focused = !s.composer_edit().focused && s.pickers().toolbar_focus == Some(Kind::Effort);
    let shown = !default || s.pickers().open == Some(Kind::Effort) || focused;
    if shown { button } else { button.group_opacity(0.0, 1.0) }
}

/// The open list, above its trigger.
pub fn picker<S: PickerHost>(s: &S, p: &Pal) -> El {
    let pk = s.pickers();
    let kind = pk.open.unwrap();
    let look = if kind == Kind::Model { LIST } else { MENU };
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = pop::surface::<S>(p, look, format!("picker-{}", kind.key()), format!("Choose {}", kind.key())).w(match kind {
        Kind::Model => 480.0 + rail::W,
        Kind::Project => 240.0,
        _ => 180.0,
    });
    if kind == Kind::Model {
        let field = pop::search_field(p, "picker-search", &pk.search, "Search models")
            .on_edit(|s: &mut S| &mut s.pickers_mut().search)
            .on_input_key(|s: &mut S, ev, tx| s.picker_key(ev, tx) || s.picker_search_key(ev, tx));
        // Desktop's trailing buttons (`select-dialog.tsx:1010-1034`); the host offers what it has.
        let trailing: Vec<El> = s.picker_model_actions().into_iter().map(|a| model_action::<S>(p, a)).collect();
        menu = menu.child(pop::search_row(p, look, "picker-search-row", rail::LEAD, field, trailing));
    }
    let mut list = pop::List::new("picker-options", look);
    let mut footer: Option<El> = None;
    let mut group = "";
    let mut stops: Vec<rail::Stop> = Vec::new();
    let mut lit_group = None;
    for item in &items {
        let is_add = kind == Kind::Project && item.key == ADD_PROJECT_KEY;
        if !is_add && !item.group.is_empty() && item.group != group {
            // The group's identity keys its header and its rail mark; the heading is what it says.
            let heading = if item.heading.is_empty() { item.group.as_str() } else { item.heading.as_str() };
            stops.push(rail::Stop { group: item.group.clone(), label: heading.to_owned(), y: list.next_y(), first: item.key.clone() });
            list.header(p, format!("group-{}", item.group), heading);
            group = &item.group;
        }
        let key = item.key.clone();
        let is_active = active.as_deref() == Some(key.as_str());
        if is_active {
            lit_group = Some(item.group.as_str());
        }
        let mut o = Opt::new(format!("option-{key}"), item.label.clone()).active(is_active).checked(!is_add && item.key == selected);
        if kind == Kind::Model {
            o = o.mark(search_match(&item.label, &pk.search.text));
        }
        if !is_add {
            o = o.slides();
        }
        if is_add {
            o = o.role("menuitem").lead(icon(&ic::PLUS, 16.0).color(p.icon_muted)).trailing(pop::hint(p, &crate::keymap::Bind::OpenProject.label()));
        } else if kind == Kind::Project
            && let Some(lead) = s.picker_project_lead(p, &item.key, &item.label)
        {
            o = o.lead(lead);
        }
        if !item.note.is_empty() {
            o = o.after(text(item.note.clone()).font(UI).color(p.faint).shrink().truncate_fade());
        }
        if item.free {
            o = o.after(pop::badge(p, "Free"));
        }
        if item.latest {
            o = o.after(pop::badge(p, "Latest"));
        }
        if kind == Kind::Model && is_active {
            let star = key.clone();
            o = o.trailing(pop::star(p, pk.prefs.is_favorite(&key), move |s: &mut S| s.toggle_favorite(&star)));
            // Last, so its efforts open right beside it.
            if let Some(effort) = s.model_effort(&key) {
                let label = if effort.is_empty() { "Default".to_owned() } else { capitalize(&effort) };
                let open = key.clone();
                o = o.trailing(pop::trailing_note(p, &label, move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    s.press_effort_chevron(&open);
                }));
            }
        }
        let hover_key = key.clone();
        let pick = key.clone();
        let row = pop::option(
            p,
            look,
            o,
            move |s: &mut S| {
                if kind == Kind::Model {
                    s.model_row_entered(&hover_key);
                } else {
                    s.pickers_mut().activate(hover_key.clone());
                }
            },
            move |s: &mut S, tx: &mut Tx| s.choose_picker_tx(&pick, tx),
        );
        let row = if kind == Kind::Model { hover::row::<S>(row, &key).children(flyout::anchor(s, &key)) } else { row };
        if is_add {
            footer = Some(row);
        } else {
            list.option(row, is_active);
        }
    }
    if kind == Kind::Model && items.is_empty() {
        // Nothing to jump to, so no rail: the words stand on the labels' edge with no empty
        // column and hairline beside the search (t1389).
        list.other(pop::empty(p, "picker-empty", "No model results", rail::LEAD + look.label_x()), pop::EMPTY_H);
    }
    list.slide(s.knobs(), p, &pk.search.text);
    let listed = !list.is_empty();
    let max_h = if kind == Kind::Model { 360.0 } else { 240.0 };
    if listed {
        let h = list.height().min(max_h);
        let scroll = list.build("picker-scroll", max_h);
        menu = menu.child(match kind {
            Kind::Model if items.is_empty() => hover::scroll::<S>(scroll.h_full()),
            Kind::Model => row()
                .key("picker-body")
                .w_full()
                .h(h)
                .shrink()
                .min_h(0.0)
                .child(rail::view::<S>(p, look, &stops, lit_group))
                .child(hover::scroll::<S>(scroll.h_full())),
            _ => scroll,
        });
    }
    if let Some(add_row) = footer {
        if listed {
            menu = menu.child(pop::divider(p, "picker-add-divider"));
        }
        menu = menu.child(col().key("picker-footer").w_full().padding(look.inset).child(add_row));
    }
    pop::place(s.knobs(), menu, kind.key(), Side::Top, Align::Start, 6.0)
}

/// Where the query first shows in a model's name, ignoring case: the one stretch Desktop's
/// `SearchHighlight` marks (`search-list.tsx:377-403`). None when it isn't there as typed, or
/// when lowercasing moves the name's offsets.
fn search_match(name: &str, query: &str) -> Option<std::ops::Range<usize>> {
    let needle = query.trim().to_lowercase();
    let hay = name.to_lowercase();
    if needle.is_empty() || hay.len() != name.len() {
        return None;
    }
    hay.find(&needle).map(|at| at..at + needle.len())
}

/// A button beside the model search: it closes the list and hands the action to the host.
fn model_action<S: PickerHost>(p: &Pal, action: ModelAction) -> El {
    let glyph = match action {
        ModelAction::Connect => &ic::PLUS,
        ModelAction::Manage => &ic::OUTLINE_SLIDERS,
    };
    let key = match action {
        ModelAction::Connect => "picker-connect",
        ModelAction::Manage => "picker-manage",
    };
    pop::search_button::<S>(p, key, glyph, action.label(), move |s: &mut S, tx: &mut Tx| {
        tx.instant();
        s.close_picker(false);
        match action {
            ModelAction::Manage => s.pickers_mut().open_manage(),
            ModelAction::Connect => s.picker_model_action(action, tx),
        }
    })
}
