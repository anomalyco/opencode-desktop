use super::*;

// ---------------------------------------------------------------------------------------------
// Composer and status bar

pub(super) fn ghost(p: &Pal, key: &str, label: &str, chevron: bool, color: Color) -> El {
    let mut r = row()
        .key(key)
        .role("button")
        .h(28.0)
        .px(8.0)
        .gap(2.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI).color(color));
    if chevron {
        r = r.child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_faint));
    }
    r
}

pub(super) fn composer_dock(s: &App, p: &Pal) -> El {
    let queue = match &s.route {
        Route::Session(id) => Some(crate::queue::panel(s, p, id, s.busy(id))),
        _ => None,
    };
    col().key("dock").w_full().center().px(24.0).pb(12.0).children(queue).child(composer(s, p))
}

/// The editor's height cap, its padding included (Desktop's `max-h-[180px]`).
pub const COMPOSER_MAX_H: f32 = 180.0;

pub fn composer_fill(p: &Pal) -> Color {
    if p.dark { p.layer1 } else { p.bg_base }
}

pub(super) fn composer(s: &App, p: &Pal) -> El {
    let selected = s.selection();
    let model = selected.model;
    let effort = if model.variant.is_empty() { "Default".into() } else { model::capitalize(&model.variant) };
    let ready = !s.composer.text.trim().is_empty() || ((s.attach.any() || s.quote.any()) && !s.add.shell);
    let busy = matches!(&s.route, Route::Session(id) if s.busy(id));
    // While dictating, Send finishes and sends with the words (dictation::submit), as on iOS.
    let stop = busy && !ready && !s.dictation.active();
    let submit = el()
        .key("submit")
        .role("button")
        .label("Send")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if ready { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| if stop { s.stop() } else { s.send(tx) })
        .child(if stop {
            el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text)
        } else {
            icon(&ic::ARROW_UP, 16.0).color(match (ready, p.dark) { (false, _) => p.icon_faint, (true, true) => p.text, (true, false) => p.q.contrast_text })
        });
    let submit = if stop { submit.label("Stop").fill(p.contrast) } else { submit };
    let composer = col()
        .key("composer")
        .w_full()
        .max_w(CONTENT_W)
        .min_h(96.0)
        .corners(if crate::queue::docked(s) { [0.0, 0.0, 12.0, 12.0] } else { [12.0; 4] })
        .fill(composer_fill(p))
        .border(0.5, p.ring)
        .shadow(1.0, 2.0, p.shadow)
        // The question dock's face springs in and out under this edge.
        .clip_soft(0.0);
    match crate::question::face(s, p) {
        Some(face) => composer.children(face),
        None => composer.children(crate::attachments::chips(s, p))
        .child(
            // Desktop's editor: 180 pt with its padding, then the text scrolls inside it, fading
            // under each edge while it hides lines. The bottom padding stays outside the scroll,
            // so a caret followed to the last line leaves the prompt's end flush.
            col().key("editor").transition(crate::question::face_transition(s)).w_full().shrink().pb(8.0).child(
                col().key("editor-scroll").w_full().shrink().max_h(COMPOSER_MAX_H - 8.0)
                .scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(16.0).scroll_margin(16.0)
                .child(col().key("editor-pad").w_full().px(16.0).pt(16.0).child(
                text_input(&s.composer, if s.add.shell { SHELL_PROMPT } else { PROMPT })
                    .key("prompt")
                    .color(p.text)
                    .w_full()
                    .min_h(40.0)
                    .animate_edit_layout()
                    .placeholder(if s.add.shell { "Enter shell command…" } else { "Ask anything, / for commands, @ for context…" }, p.faint)
                    .caret(p.text, p.selection)
                    .on_edit(|s: &mut App| &mut s.composer)
                    .pill_style(crate::mentions::badge::pill_style(p))
                    .on_input_change(|s: &mut App, tx| { crate::quote::typed(s, tx); s.mentions_changed(tx); s.slash_changed(tx) })
                    .on_input_key(|s: &mut App, ev, tx| {
                        if crate::rewind::menu_key(s, ev, tx) || s.fork_key(ev, tx) || s.mention_key(ev, tx) || s.add_key(ev, tx) {
                            return true;
                        }
                        if ev.key == Key::Escape && !s.route_busy() && crate::status_toast::key(s, ev, tx) {
                            return true;
                        }
                        if ev.key == Key::Enter && ev.mods.cmd {
                            s.submit(true, tx);
                            return true;
                        }
                        s.shortcut(ev, tx) || s.picker_key(ev, tx) || s.history_key(ev, tx)
                    })
                    .on_input_paste(crate::attachments::paste)
                    .on_submit(|s: &mut App, tx: &mut Tx| s.send(tx)),
            ))),
        )
        .child(
            row()
                .key("toolbar")
                .transition(crate::question::face_transition(s))
                .h(44.0)
                .px(8.0)
                .gap(4.0)
                .center_y()
                .child(crate::add_menu::trigger(s, p))
                .children(s.settings.show_agent.then(|| picker_trigger(s, p, Kind::Agent, &model::capitalize(&selected.agent), p.muted)))
                .child(picker_trigger(s, p, Kind::Model, &s.pickers.model_name(&model), p.muted))
                // "Default" is the absence of a choice, so it reads fainter than a picked effort.
                .children(s.has_effort().then(|| {
                    picker_trigger(s, p, Kind::Effort, &effort, if model.variant.is_empty() { p.faint } else { p.muted })
                }))
                .child(spacer().key("room").h(28.0).min_w(0.0).justify_end().center_y().children((busy && ready).then(|| {
                    let room = rune::memo::layout_size("room").map_or(f32::INFINITY, |[w, _]| w / s.zoom);
                    crate::queue::alternate_hint(p, room)
                }).flatten()))
                .children(crate::auto_accept::indicator(s, p))
                .children(crate::dictation::toolbar(s, p))
                .child(submit),
        )
        .children(crate::dictation::status(s, p)),
    }
}

pub(super) fn picker_trigger(s: &App, p: &Pal, kind: Kind, label: &str, color: Color) -> El {
    // A new label resizes it and moves its neighbours in the choice, with no glide.
    let mut button = ghost(p, kind.key(), label, true, color).label(format!("Choose {}", kind.key()))
        .motion(Spring::NONE);
    if !s.composer.focused && s.pickers.toolbar_focus == Some(kind) {
        button = button.fill(p.hover);
    }
    button.on_press_tx(move |s: &mut App, tx: &mut Tx| { s.open_picker(kind); if s.pickers.open.is_some() { tx.instant(); } })
}

pub(super) fn picker(s: &App, p: &Pal) -> El {
    use crate::pickers::ADD_PROJECT_KEY;
    use crate::popover::{self as pop, LIST, MENU, Opt};
    let kind = s.pickers.open.unwrap();
    let look = if kind == Kind::Model { LIST } else { MENU };
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = pop::surface(p, look, format!("picker-{}", kind.key()), format!("Choose {}", kind.key()))
        .w(match kind { Kind::Model => 320.0, Kind::Project => 240.0, _ => 180.0 });
    if kind == Kind::Model {
        let field = pop::search_field(p, "picker-search", &s.pickers.search, "Search models")
            .on_edit(|s: &mut App| &mut s.pickers.search)
            .on_input_key(|s: &mut App, ev, tx| s.picker_key(ev, tx) || s.shortcut(ev, tx));
        menu = menu.child(pop::search_row(p, look, "picker-search-row", field));
    }
    let mut list = pop::List::new("picker-options", look);
    let mut footer: Option<El> = None;
    let mut group = "";
    for item in &items {
        let is_add = kind == Kind::Project && item.key == ADD_PROJECT_KEY;
        if !is_add && !item.group.is_empty() && item.group != group {
            list.header(p, format!("group-{}", item.group), &item.group);
            group = &item.group;
        }
        let key = item.key.clone();
        let is_active = active.as_deref() == Some(key.as_str());
        let mut o = Opt::new(format!("option-{key}"), item.label.clone()).active(is_active).checked(!is_add && item.key == selected);
        if is_add {
            o = o.role("menuitem").lead(icon(&ic::PLUS, 16.0).color(p.icon_muted)).trailing(pop::hint(p, "⌘O"));
        } else if kind == Kind::Project {
            o = o.lead(crate::icons::badge(s, p, s.project(&item.key), &item.label));
        }
        if !item.note.is_empty() {
            o = o.after(text(item.note.clone()).font(UI).color(p.faint).shrink().truncate_fade());
        }
        if item.free {
            o = o.after(pop::badge(p, "Free"));
        }
        if kind == Kind::Model && is_active {
            let star = key.clone();
            o = o.trailing(pop::star(p, s.pickers.prefs.is_favorite(&key), move |s: &mut App| s.toggle_favorite(&star)));
        }
        let hover_key = key.clone();
        let row = pop::option(p, look, o, move |s: &mut App| s.pickers.activate(hover_key.clone()), move |s: &mut App, tx: &mut Tx| s.choose_picker_tx(&key, tx));
        if is_add {
            footer = Some(row);
        } else {
            list.row(row);
        }
    }
    if kind == Kind::Model && items.is_empty() {
        list.other(pop::empty(p, look, "picker-empty", "No models found"), pop::EMPTY_H);
    }
    let listed = !list.is_empty();
    let max_h = if kind == Kind::Model { 360.0 } else { 240.0 };
    if listed {
        menu = menu.child(list.build("picker-scroll", max_h));
    }
    if let Some(add_row) = footer {
        if listed {
            menu = menu.child(pop::divider(p, "picker-add-divider"));
        }
        menu = menu.child(col().key("picker-footer").w_full().padding(look.inset).child(add_row));
    }
    pop::place(menu, kind.key(), Side::Top, Align::Start, 6.0)
}

pub(super) fn status_bar(s: &App, p: &Pal, info: Option<&SessionInfo>) -> El {
    let dir = info.map_or(String::new(), |i| i.directory.replace("/Users/demo", "~").replace(&home_dir(), "~"));
    let live = s.live();
    row()
        .key("status")
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .child(icon(&ic::FOLDER, 14.0).color(p.icon_faint))
        .child(text(dir).font(UI_SMALL).color(p.faint).truncate_fade())
        .child(spacer())
        .child(el().size(6.0, 6.0).radius(3.0).fill(if live { p.success } else { p.icon_faint }))
        .child(text(s.status.clone()).font(UI_SMALL).color(p.faint))
}
