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

pub fn composer_fill(p: &Pal) -> Color {
    if p.dark { p.layer1 } else { p.bg_base }
}

pub(super) fn composer(s: &App, p: &Pal) -> El {
    let selected = s.selection();
    let model = selected.model;
    let effort = if model.variant.is_empty() { "Default".into() } else { model::capitalize(&model.variant) };
    let ready = !s.composer.text.trim().is_empty() || (s.attach.any() && !s.add.shell);
    let busy = matches!(&s.route, Route::Session(id) if s.busy(id));
    let stop = busy && !ready;
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
            icon(&ic::ARROW_UP, 16.0).color(if ready { p.text } else { p.icon_faint })
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
            col().key("editor").transition(crate::question::face_transition(s)).w_full().px(16.0).pt(16.0).pb(8.0).child(
                text_input(&s.composer, if s.add.shell { SHELL_PROMPT } else { PROMPT })
                    .key("prompt")
                    .w_full()
                    .min_h(40.0)
                    .animate_edit_layout()
                    .placeholder(if s.add.shell { "Enter shell command…" } else { "Ask anything, / for commands, @ for context…" }, p.faint)
                    .caret(p.text, p.selection)
                    .on_edit(|s: &mut App| &mut s.composer)
                    .pill_style(crate::mentions::pill_style(p))
                    .on_input_change(|s: &mut App, tx| s.mentions_changed(tx))
                    .on_input_key(|s: &mut App, ev, tx| {
                        if s.mention_key(ev, tx) || s.add_key(ev, tx) {
                            return true;
                        }
                        if ev.key == Key::Enter && ev.mods.cmd {
                            s.submit(true, tx);
                            return true;
                        }
                        s.shortcut(ev, tx) || s.picker_key(ev, tx)
                    })
                    .on_input_paste(crate::attachments::paste)
                    .on_submit(|s: &mut App, tx: &mut Tx| s.send(tx)),
            ),
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
                .child(picker_trigger(s, p, Kind::Model, &s.pickers.models.iter()
                    .find(|m| m.id == model.id && m.provider == model.provider).map_or_else(|| model.display(), |m| m.name.clone()), p.muted))
                // "Default" is the absence of a choice, so it reads fainter than a picked effort.
                .children(s.has_effort().then(|| {
                    picker_trigger(s, p, Kind::Effort, &effort, if model.variant.is_empty() { p.faint } else { p.muted })
                }))
                .child(spacer())
                .children((busy && ready).then(|| crate::queue::alternate_hint(p)))
                .child(submit),
        ),
    }
}

pub(super) fn picker_trigger(s: &App, p: &Pal, kind: Kind, label: &str, color: Color) -> El {
    // A new label resizes it and moves its neighbours in the choice, with no glide.
    let mut button = ghost(p, kind.key(), label, true, color).label(format!("Choose {}", kind.key()))
        .motion(no_motion().enter);
    if !s.composer.focused && s.pickers.toolbar_focus == Some(kind) {
        button = button.fill(p.hover);
    }
    if !s.live() {
        button = button.on_press_tx(move |s: &mut App, tx: &mut Tx| { s.open_picker(kind); if s.pickers.open.is_some() { tx.instant(); } });
    }
    button
}

/// Overrides Rune's default 0.2 s fade: menus and palettes appear and go on the keypress or
/// click. (`tx.instant()` would also skip it, but an instant unmount of a focused field drops
/// the composer's restored focus.)
pub fn no_motion() -> rune::SpringPair {
    rune::SpringPair { enter: rune::Spring::new(0.001, 0.0), exit: rune::Spring::new(0.001, 0.0) }
}

pub(super) fn picker(s: &App, p: &Pal) -> El {
    let kind = s.pickers.open.unwrap();
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = col().key(format!("picker-{}", kind.key())).role("menu").label(format!("Choose {}", kind.key()))
        .anchored(kind.key(), if kind == Kind::Project { Side::Bottom } else { Side::Top }, Align::Start, 6.0)
        .w(match kind { Kind::Model => 284.0, Kind::Project => 240.0, _ => 180.0 })
        .padding(2.0).radius(6.0).fill(p.layer1).border(0.5, p.border_muted)
        .shadow(4.0, 12.0, p.shadow).transition(rune::Transition::opacity().springs(no_motion()))
        // Its height and rows follow the filter in the keystroke.
        .motion(no_motion().enter)
        // Empty space in the menu consumes pointer presses instead of dismissing it.
        .on_press(|_: &mut App| {});
    if kind == Kind::Model {
        menu = menu.child(row().key("picker-search-row").h(28.0).pl(10.0).pr(4.0).gap(8.0).center_y()
            .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
            .child(text_input(&s.pickers.search, UI).key("picker-search").grow(1.0).shrink().h(28.0).animate_edit_layout()
                .placeholder("Search models", p.faint).caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.pickers.search)
                .on_input_key(|s: &mut App, ev, tx| s.shortcut(ev, tx) || s.picker_key(ev, tx))))
            .child(el().h(0.5).w_full().fill(p.border_muted));
    }
    // Filtering is instant, like Desktop's list: rows come and go in the keystroke.
    let appear = rune::Transition::opacity().springs(no_motion());
    let mut list = col().key("picker-options").w_full();
    let mut group = "";
    let mut height = 0.0_f32;
    for item in &items {
        if !item.group.is_empty() && item.group != group {
            list = list.child(col().key(format!("group-{}", item.group)).h(24.0).transition(appear.clone())
                .child(text(item.group.clone()).font(UI_SMALL).color(p.faint).h(24.0).px(10.0).pt(4.0)));
            height += 24.0;
            group = &item.group;
        }
        let key = item.key.clone();
        let hover_key = key.clone();
        let mut option = row().key(format!("option-{key}")).role("menuitemradio").label(item.label.clone())
            .h(28.0).w_full().px(10.0).gap(8.0).center_y().radius(4.0).transition(appear.clone())
            .fill(if active.as_deref() == Some(&key) { p.hover } else { Color::TRANSPARENT })
            .hover(p.hover).press(p.pressed)
            // Not `on_hover`: its commit would animate the highlight's fill from row to row.
            .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                if inside {
                    tx.instant();
                    s.pickers.activate(hover_key.clone());
                }
            })
            .on_press(move |s: &mut App| s.choose_picker(&key))
            .child(text(item.label.clone()).font(UI).color(p.text)).child(spacer());
        if item.key == selected { option = option.child(icon(&ic::CHECK_SMALL, 16.0).color(p.icon)); }
        list = list.child(option);
        height += 28.0;
    }
    if kind == Kind::Model && items.is_empty() {
        list = list.child(col().key("picker-empty").h(48.0).transition(appear)
            .child(text("No models found").font(UI).color(p.faint).h(48.0).px(10.0).pt(16.0)));
        height = 48.0;
    }
    menu.child(col().key("picker-scroll").w_full().h(height.min(220.0))
        .scroll(ScrollSpec { stick_bottom: false }).clip_fade(0.0, 8.0, 0.0, 8.0).child(list))
}

pub(super) fn status_bar(s: &App, p: &Pal, info: Option<&SessionInfo>) -> El {
    let dir = info.map_or(String::new(), |i| i.directory.replace("/Users/demo", "~").replace(&home_dir(), "~"));
    let e = edge::ALL[s.edge];
    let live = s.live();
    row()
        .key("status")
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .child(icon(&ic::FOLDER, 14.0).color(p.icon_faint))
        .child(text(dir).font(UI_SMALL).color(p.faint).truncate())
        .child(spacer())
        .child(
            row()
                .key("edge")
                .role("button")
                .label("Streaming edge")
                .h(22.0)
                .px(6.0)
                .center_y()
                .radius(4.0)
                .hover(p.hover)
                .on_press(|s: &mut App| s.edge = (s.edge + 1) % edge::ALL.len())
                .child(text(format!("Edge: {}", e.name)).font(UI_SMALL).color(p.faint)),
        )
        .child(el().size(6.0, 6.0).radius(3.0).fill(if live { p.success } else { p.icon_faint }))
        .child(text(s.status.clone()).font(UI_SMALL).color(p.faint))
}
