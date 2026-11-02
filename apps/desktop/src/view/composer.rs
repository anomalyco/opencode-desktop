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
    let (queue, status) = match &s.route {
        Route::Session(id) => (Some(crate::queue::panel(s, p, id, s.busy(id))), crate::subagents::reply::status_line(s, p, id)),
        _ => (None, None),
    };
    // A subagent's page keeps its status line under the composer, in the dock's bottom inset.
    let pb = if status.is_some() { 0.0 } else { crate::view::DOCK_SIDE };
    col().key("dock").w_full().center().px(crate::view::DOCK_SIDE).pb(pb).children(queue).child(composer(s, p)).children(status)
}

/// The editor's height cap, its padding included (Desktop's `max-h-[180px]`).
pub const COMPOSER_MAX_H: f32 = 180.0;

thread_local! {
    static WHOLE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether the composer showed its whole prompt when last laid out: a longer one scrolls inside
/// the editor, its words starting out of sight.
pub fn shows_whole_prompt() -> bool {
    WHOLE.get()
}

pub fn composer_fill(p: &Pal) -> Color {
    if p.dark { p.layer1 } else { p.bg_base }
}

pub(super) fn composer(s: &App, p: &Pal) -> El {
    let (pad, shown) = (rune::memo::layout_with("editor-pad", |r| r[3]), rune::memo::layout_with("editor-scroll", |r| r[3]));
    WHOLE.set(!matches!((pad, shown), (Some(pad), Some(shown)) if pad > shown + 0.5));
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
        // The fill changes at once; Stop's square and Send's arrow change places (stop::glyph_swap).
        .motion(rune::Spring::NONE)
        .motion_intent(rune::MotionIntent::CUT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| if stop { crate::stop::press(s, tx) } else { s.send(tx) })
        .child(if stop {
            el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text)
        } else {
            icon(&ic::ARROW_UP, 16.0).key("send-glyph").color(match (ready, p.dark) { (false, _) => p.icon_faint, (true, true) => p.text, (true, false) => p.q.contrast_text })
        }.transition(crate::stop::glyph_swap()));
    let submit = if stop { submit.label("Stop").fill(p.contrast) } else { submit };
    let composer = col()
        .key("composer")
        .w_full()
        .max_w(crate::view::DOCK_W)
        .min_h(88.0)
        .corners(if crate::queue::docked(s) { [0.0, 0.0, 12.0, 12.0] } else { [12.0; 4] })
        .fill(composer_fill(p))
        .border(0.5, p.ring)
        .shadow(1.0, 2.0, p.shadow)
        // A dock's face (a question, a permission) springs in and out under this edge.
        .clip_soft(0.0);
    match crate::question::face(s, p).or_else(|| crate::permission::face(s, p)) {
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
                    .min_h(20.0)
                    .animate_edit_layout()
                    .placeholder(if s.add.shell { "Enter shell command…" } else { "Ask anything, / for commands, @ for context…" }, p.faint)
                    .caret(p.text, p.selection)
                    .on_edit(|s: &mut App| &mut s.composer)
                    .pill_style(crate::mentions::badge::pill_style(p))
                    .on_input_change(|s: &mut App, tx| { crate::quote::typed(s, tx); s.mentions_changed(tx); s.slash_changed(tx) })
                    .on_input_key(|s: &mut App, ev, tx| {
                        if crate::rewind::menu_key(s, ev, tx) || s.fork_key(ev, tx) || s.mention_key(ev, tx) || s.add_key(ev, tx) || crate::subagents::reply::esc(s, ev, tx) || crate::stop::key(s, ev, tx) {
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
                .children((s.settings.show_agent || s.subagent_page().is_some()).then(|| picker_trigger(s, p, Kind::Agent, &model::capitalize(&selected.agent), p.muted)))
                .child(picker_trigger(s, p, Kind::Model, &s.pickers.model_name(&model), p.muted))
                // "Default" is the absence of a choice, so it reads fainter than a picked effort.
                .children(s.has_effort().then(|| {
                    picker_trigger(s, p, Kind::Effort, &effort, if model.variant.is_empty() { p.faint } else { p.muted })
                }))
                .child(spacer().key("room").h(28.0).min_w(0.0).justify_end().center_y().children((busy && ready).then(|| {
                    let room = rune::memo::layout_size("room").map_or(f32::INFINITY, |[w, _]| w);
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
    button.on_press_tx(move |s: &mut App, tx: &mut Tx| { s.open_picker(kind); if s.pickers.open.is_some() { crate::popover::motion::opening(&s.knobs, tx); } })
}

pub(super) fn picker(s: &App, p: &Pal) -> El {
    use crate::pickers::ADD_PROJECT_KEY;
    use crate::model_picker::hover;
    use crate::popover::{self as pop, LIST, MENU, Opt};
    let kind = s.pickers.open.unwrap();
    let look = if kind == Kind::Model { LIST } else { MENU };
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = pop::surface(p, look, format!("picker-{}", kind.key()), format!("Choose {}", kind.key()))
        .w(match kind { Kind::Model => 480.0 + crate::model_picker::rail::W, Kind::Project => 240.0, _ => 180.0 });
    if kind == Kind::Model {
        let field = pop::search_field(p, "picker-search", &s.pickers.search, "Search models")
            .on_edit(|s: &mut App| &mut s.pickers.search)
            .on_input_key(|s: &mut App, ev, tx| s.picker_key(ev, tx) || s.shortcut(ev, tx));
        menu = menu.child(pop::search_row(p, look, "picker-search-row", crate::model_picker::rail::LEAD, field));
    }
    let mut list = pop::List::new("picker-options", look);
    let mut footer: Option<El> = None;
    let mut group = "";
    let mut stops: Vec<crate::model_picker::rail::Stop> = Vec::new();
    let mut lit_group = None;
    for item in &items {
        let is_add = kind == Kind::Project && item.key == ADD_PROJECT_KEY;
        if !is_add && !item.group.is_empty() && item.group != group {
            stops.push(crate::model_picker::rail::Stop { group: item.group.clone(), y: list.next_y(), first: item.key.clone() });
            list.header(p, format!("group-{}", item.group), &item.group);
            group = &item.group;
        }
        let key = item.key.clone();
        let is_active = active.as_deref() == Some(key.as_str());
        if is_active { lit_group = Some(item.group.as_str()); }
        let mut o = Opt::new(format!("option-{key}"), item.label.clone()).active(is_active).checked(!is_add && item.key == selected);
        if !is_add {
            o = o.slides();
        }
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
            // Last, so its efforts open right beside it.
            if let Some(effort) = s.model_effort(&key) {
                let label = if effort.is_empty() { "Default".to_owned() } else { model::capitalize(&effort) };
                let open = key.clone();
                o = o.trailing(pop::trailing_note(p, &label, move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.press_effort_chevron(&open);
                }));
            }
        }
        let hover_key = key.clone();
        let row = pop::option(p, look, o, move |s: &mut App| {
            if kind == Kind::Model { s.model_row_entered(&hover_key); } else { s.pickers.activate(hover_key.clone()); }
        }, {
            let key = key.clone();
            move |s: &mut App, tx: &mut Tx| s.choose_picker_tx(&key, tx)
        });
        let row = if kind == Kind::Model { hover::row(row, &key).children(crate::model_picker::flyout::anchor(s, &key)) } else { row };
        if is_add {
            footer = Some(row);
        } else {
            list.option(row, is_active);
        }
    }
    if kind == Kind::Model && items.is_empty() {
        list.other(pop::empty(p, look, "picker-empty", "No models found"), pop::EMPTY_H);
    }
    list.slide(&s.knobs, p, &s.pickers.search.text);
    let listed = !list.is_empty();
    let max_h = if kind == Kind::Model { 360.0 } else { 240.0 };
    if listed {
        let h = list.height().min(max_h);
        let scroll = list.build("picker-scroll", max_h);
        menu = menu.child(match kind {
            Kind::Model => row().key("picker-body").w_full().h(h).shrink().min_h(0.0)
                .child(crate::model_picker::rail::view(p, look, &stops, lit_group))
                .child(hover::scroll(scroll.h_full())),
            _ => scroll,
        });
    }
    if let Some(add_row) = footer {
        if listed {
            menu = menu.child(pop::divider(p, "picker-add-divider"));
        }
        menu = menu.child(col().key("picker-footer").w_full().padding(look.inset).child(add_row));
    }
    pop::place(&s.knobs, menu, kind.key(), Side::Top, Align::Start, 6.0)
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
