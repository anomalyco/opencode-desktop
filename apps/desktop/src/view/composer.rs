use opencode_kit::client::PathAttachment;
use opencode_kit::model::Attachment;
use rune::{KeyEvent, TextEdit, Transition};
#[cfg(test)]
pub use session_view::composer::COMPOSER_MAX_H;
pub use session_view::composer::{ComposerHost, composer_fill, shows_whole_prompt};

use super::narrow::{self, Agent};
use super::*;

// ---------------------------------------------------------------------------------------------
// Composer and status bar

pub(super) fn ghost(p: &Pal, key: &str, label: &str, chevron: bool, color: Color) -> El {
    session_view::composer::ghost(p, key, label, chevron, color)
}

pub(super) fn composer_dock(s: &App, p: &Pal) -> El {
    let (queue, status) = match &s.route {
        Route::Session(id) => (
            Some(crate::queue::panel(s, p, id, s.busy(id))),
            crate::subagents::reply::status_line(s, p, id),
        ),
        _ => (None, None),
    };
    // A subagent's page keeps its status line under the composer, in the dock's bottom inset.
    let pb = if status.is_some() { 0.0 } else { crate::view::DOCK_SIDE };
    col()
        .key("dock")
        .w_full()
        .center()
        .px(crate::view::DOCK_SIDE)
        .pb(pb)
        .children(queue)
        .child(composer(s, p))
        .children(status)
}

pub(super) fn composer(s: &App, p: &Pal) -> El {
    session_view::composer::composer(s, p)
}

/// The composer, keyed `{scope}-composer` when `scope` isn't empty (copies on one stage).
pub(crate) fn composer_in(s: &App, p: &Pal, scope: &str) -> El {
    session_view::composer::composer_in(s, p, scope)
}

impl session_view::dock::DockHost for App {
    fn dock(&self) -> &session_view::dock::Dock {
        &self.question
    }
    fn dock_mut(&mut self) -> &mut session_view::dock::Dock {
        &mut self.question
    }
    fn dock_pending(&self, id: &str) -> Vec<opencode_kit::model::Pending> {
        self.pending_for(id).into_iter().cloned().collect()
    }
    fn dock_parent(&self, id: &str) -> Option<String> {
        self.session(id).and_then(|i| i.parent.clone())
    }
    fn dock_auto_accept(&self) -> bool {
        self.settings.auto_accept
    }
    fn dock_reply_permission(&mut self, id: &str, decision: &str) {
        self.reply_permission(id, decision);
    }
    fn dock_submit_form(&mut self, id: &str) {
        self.submit_form(id);
    }
    fn dock_dismiss_form(&mut self, id: &str) {
        self.dismiss_form(id);
    }
    fn dock_waiting(&self, id: &str) -> bool {
        self.pending.iter().any(|p| p.id() == id)
    }
    fn dock_refocus(&self) -> bool {
        self.subagent_page().is_none()
    }
    fn dock_shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }
}

impl ComposerHost for App {
    fn composer_edit(&self) -> &TextEdit {
        &self.composer
    }
    fn composer_edit_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }
    fn composer_busy(&self) -> bool {
        matches!(&self.route, Route::Session(id) if self.busy(id))
    }
    fn composer_send(&mut self, tx: &mut Tx) {
        self.send(tx);
    }
    fn composer_stop(&mut self, tx: &mut Tx) {
        crate::stop::press(self, tx);
    }
    fn composer_attachments(&self) -> &[Attachment] {
        &self.attach.files
    }
    fn composer_remove_attachment(&mut self, id: &str) {
        self.attach.files.retain(|f| crate::attachments::identity(f) != id);
    }
    fn composer_paste_attachment(&mut self, a: Attachment) -> bool {
        self.attach.add(a);
        true
    }
    fn composer_pick_attachment(&mut self, tx: &mut Tx) {
        self.pick_attachment(tx);
    }
    fn composer_path_attachments(&self) -> &[PathAttachment] {
        &self.attach.paths
    }
    fn composer_remove_path_attachment(&mut self, path: &str) {
        self.attach.paths.retain(|f| f.path != path);
    }
    fn composer_uploading(&self) -> &[String] {
        &self.attach.uploading
    }
    fn composer_quote_chips(&self, p: &Pal) -> Vec<El> {
        crate::quote_cards::chips(self, p).into_iter().collect()
    }
    fn composer_has_extra_payload(&self) -> bool {
        self.quote.any()
    }
    fn composer_placeholder(&self) -> String {
        if self.add.shell {
            "Enter shell command…".into()
        } else {
            "Ask anything, / for commands, @ for context…".into()
        }
    }
    fn composer_shell_mode(&self) -> bool {
        self.add.shell
    }
    fn composer_dictating(&self) -> bool {
        self.dictation.active()
    }
    fn composer_docked_top(&self) -> bool {
        crate::queue::docked(self)
    }
    fn composer_face(&self, p: &Pal) -> Option<Vec<El>> {
        session_view::dock::face(self, p)
    }
    fn composer_face_transition(&self) -> Transition {
        crate::question::face_transition(self)
    }
    fn composer_toolbar_items(&self, p: &Pal, scope: &str, queues: bool) -> Vec<El> {
        toolbar_items(self, p, scope, queues)
    }
    fn composer_below_toolbar(&self, p: &Pal) -> Option<El> {
        crate::dictation::status(self, p)
    }
    fn composer_on_change(&mut self, tx: &mut Tx) {
        crate::quote::typed(self, tx);
        self.mentions_changed(tx);
        self.slash_changed(tx);
    }
    fn composer_on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if crate::rewind::menu_key(self, ev, tx)
            || self.fork_key(ev, tx)
            || self.mention_key(ev, tx)
            || self.add_key(ev, tx)
            || crate::subagents::reply::esc(self, ev, tx)
            || crate::stop::key(self, ev, tx)
        {
            return true;
        }
        if ev.key == Key::Escape && !self.route_busy() && crate::status_toast::key(self, ev, tx) {
            return true;
        }
        if ev.key == Key::Enter && ev.mods.cmd {
            self.submit(true, tx);
            return true;
        }
        self.shortcut(ev, tx) || self.picker_key(ev, tx) || self.history_key(ev, tx)
    }
}

/// The items before `submit` in the bar under the editor. In a narrow window its pickers fold, a
/// step at a time (`narrow::Toolbar`), so Send never clips (p263).
fn toolbar_items(s: &App, p: &Pal, scope: &str, queues: bool) -> Vec<El> {
    let selected = s.selection();
    let model = selected.model;
    let effort = if model.variant.is_empty() {
        "Default".into()
    } else {
        model::capitalize(&model.variant)
    };
    let agent = (s.settings.show_agent || s.subagent_page().is_some()).then(|| model::capitalize(&selected.agent));
    let model_name = s.pickers.model_name(&model);
    let mic = crate::dictation::toolbar(s, p);
    let dictating = s.dictation.active();
    let fold = folds(
        s,
        scope,
        agent.as_deref(),
        &model_name,
        s.has_effort().then_some(effort.as_str()),
        mic.is_some(),
        dictating,
    );
    narrow::note_toolbar(fold);
    let agent = agent.and_then(|name| match fold.agent {
        Agent::Name => Some(picker_trigger(s, p, Kind::Agent, &name, p.muted, fold.chevrons)),
        Agent::Icon => Some(agent_icon(s, p, &name)),
        Agent::Gone => None,
    });
    let mut out = vec![crate::add_menu::trigger(s, p)];
    out.extend(agent);
    out.push(picker_trigger(s, p, Kind::Model, &model_name, p.muted, fold.chevrons).shrink().min_w(0.0));
    // "Default" is the absence of a choice, so it reads fainter than a picked effort.
    if s.has_effort() && fold.effort {
        out.push(picker_trigger(
            s,
            p,
            Kind::Effort,
            &effort,
            if model.variant.is_empty() { p.faint } else { p.muted },
            fold.chevrons,
        ));
    }
    out.push(
        spacer()
            .key("room")
            .h(28.0)
            .min_w(0.0)
            .justify_end()
            .center_y()
            .children(
                queues
                    .then(|| {
                        let room = rune::memo::layout_size("room").map_or(f32::INFINITY, |[w, _]| w);
                        crate::queue::alternate_hint(p, room)
                    })
                    .flatten(),
            ),
    );
    out.extend(crate::auto_accept::indicator(s, p));
    out.extend(mic.filter(|_| fold.mic || dictating));
    out
}

/// How far the toolbar folds to fit the width the composer had at the last layout.
fn folds(
    s: &App,
    scope: &str,
    agent: Option<&str>,
    model: &str,
    effort: Option<&str>,
    mic: bool,
    dictating: bool,
) -> narrow::Toolbar {
    let room = rune::memo::layout_with(&super::header::scoped(scope, "composer"), |r| r[2] - 16.0);
    let label = |text: &str, chevron: bool| 16.0 + narrow::text_w(text, UI) + if chevron { 18.0 } else { 0.0 };
    let need = |k: usize| {
        let t = narrow::Toolbar::after(k);
        // The add button, the room, Send, and auto-accept's mark when it's on.
        let mut items = vec![28.0, 0.0, 28.0];
        if s.settings.auto_accept {
            items.push(28.0);
        }
        if mic && (t.mic || dictating) {
            items.push(28.0);
        }
        match (agent, t.agent) {
            (Some(name), Agent::Name) => items.push(label(name, t.chevrons)),
            (Some(_), Agent::Icon) => items.push(28.0),
            _ => {}
        }
        let whole = label(model, t.chevrons);
        items.push(if t.model_whole { whole } else { whole.min(narrow::MODEL_FLOOR) });
        if let Some(effort) = effort.filter(|_| t.effort) {
            items.push(label(effort, t.chevrons));
        }
        items.iter().sum::<f32>() + 4.0 * (items.len() - 1) as f32
    };
    narrow::Toolbar::after(narrow::steps(room, narrow::Toolbar::FOLDS, need))
}

/// The agent's picker folded to its mark; it still opens the agent list.
fn agent_icon(s: &App, p: &Pal, name: &str) -> El {
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
    if !s.composer.focused && s.pickers.toolbar_focus == Some(Kind::Agent) {
        b = b.fill(p.hover);
    }
    b.on_press_tx(|s: &mut App, tx: &mut Tx| {
        s.open_picker(Kind::Agent);
        if s.pickers.open.is_some() {
            crate::popover::motion::opening(&s.knobs, tx);
        }
    })
}

pub(super) fn picker_trigger(s: &App, p: &Pal, kind: Kind, label: &str, color: Color, chevron: bool) -> El {
    // A new label resizes it and moves its neighbours in the choice, with no glide.
    let mut button = ghost(p, kind.key(), label, chevron, color)
        .label(format!("Choose {}", kind.key()))
        .motion(Spring::NONE);
    if !s.composer.focused && s.pickers.toolbar_focus == Some(kind) {
        button = button.fill(p.hover);
    }
    button.on_press_tx(move |s: &mut App, tx: &mut Tx| {
        s.open_picker(kind);
        if s.pickers.open.is_some() {
            crate::popover::motion::opening(&s.knobs, tx);
        }
    })
}

pub(super) fn picker(s: &App, p: &Pal) -> El {
    use crate::model_picker::hover;
    use crate::pickers::ADD_PROJECT_KEY;
    use crate::popover::{self as pop, LIST, MENU, Opt};
    let kind = s.pickers.open.unwrap();
    let look = if kind == Kind::Model { LIST } else { MENU };
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = pop::surface(p, look, format!("picker-{}", kind.key()), format!("Choose {}", kind.key())).w(
        match kind {
            Kind::Model => 480.0 + crate::model_picker::rail::W,
            Kind::Project => 240.0,
            _ => 180.0,
        },
    );
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
            stops.push(crate::model_picker::rail::Stop {
                group: item.group.clone(),
                y: list.next_y(),
                first: item.key.clone(),
            });
            list.header(p, format!("group-{}", item.group), &item.group);
            group = &item.group;
        }
        let key = item.key.clone();
        let is_active = active.as_deref() == Some(key.as_str());
        if is_active {
            lit_group = Some(item.group.as_str());
        }
        let mut o = Opt::new(format!("option-{key}"), item.label.clone())
            .active(is_active)
            .checked(!is_add && item.key == selected);
        if !is_add {
            o = o.slides();
        }
        if is_add {
            o = o
                .role("menuitem")
                .lead(icon(&ic::PLUS, 16.0).color(p.icon_muted))
                .trailing(pop::hint(p, "⌘O"));
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
            o = o.trailing(pop::star(p, s.pickers.prefs.is_favorite(&key), move |s: &mut App| {
                s.toggle_favorite(&star)
            }));
            // Last, so its efforts open right beside it.
            if let Some(effort) = s.model_effort(&key) {
                let label = if effort.is_empty() {
                    "Default".to_owned()
                } else {
                    model::capitalize(&effort)
                };
                let open = key.clone();
                o = o.trailing(pop::trailing_note(p, &label, move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.press_effort_chevron(&open);
                }));
            }
        }
        let hover_key = key.clone();
        let row = pop::option(
            p,
            look,
            o,
            move |s: &mut App| {
                if kind == Kind::Model {
                    s.model_row_entered(&hover_key);
                } else {
                    s.pickers.activate(hover_key.clone());
                }
            },
            {
                let key = key.clone();
                move |s: &mut App, tx: &mut Tx| s.choose_picker_tx(&key, tx)
            },
        );
        let row = if kind == Kind::Model {
            hover::row(row, &key).children(crate::model_picker::flyout::anchor(s, &key))
        } else {
            row
        };
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
            Kind::Model => row()
                .key("picker-body")
                .w_full()
                .h(h)
                .shrink()
                .min_h(0.0)
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
    let dir = info.map_or(String::new(), |i| {
        i.directory.replace("/Users/demo", "~").replace(&home_dir(), "~")
    });
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
