use opencode_kit::client::PathAttachment;
use opencode_kit::model::Attachment;
use rune::{KeyEvent, TextEdit, Transition};
#[cfg(test)]
pub use session_view::composer::COMPOSER_MAX_H;
pub use session_view::composer::{ComposerHost, composer_fill, shows_whole_prompt};

use super::narrow::{self, Agent};
use super::*;
use opencode_i18n::t;

// ---------------------------------------------------------------------------------------------
// Composer and status bar

pub(super) fn composer_dock(s: &App, p: &Pal) -> El {
    let (queue, status) = match &s.route {
        Route::Session(id) => (
            Some(crate::queue::panel(s, p, id, s.busy(id))),
            crate::subagents::reply::status_line(s, p, id),
        ),
        _ => (None, None),
    };
    // Plugins' composer pills float over a root session's composer (`ComposerPill`).
    let pills = match &s.route {
        Route::Session(id) if s.session(id).is_none_or(|i| i.parent.is_none()) => crate::plugin_host::composer_pills(s, p, id),
        _ => None,
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
        .children(pills)
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
    fn dock_new_question_ui(&self) -> bool {
        self.settings.new_question_ui
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
        crate::quote_cards::chips(self, p).into_iter().chain(crate::add_selection::chips(self, p)).collect()
    }
    fn composer_has_extra_payload(&self) -> bool {
        self.quote.any()
    }
    /// Desktop's (`composer/model.ts:113-120`): a shell example, a follow-up while the session
    /// works or has prompts queued, else the question.
    fn composer_placeholder(&self) -> String {
        let queued = matches!(&self.route, Route::Session(id) if self.inbox_for(id).is_some_and(|i| i.queued().next().is_some()));
        if self.add.shell {
            t!("prompt.placeholder.shell", example = "git status")
        } else if self.composer_busy() || queued {
            t!("ui.promptInput.placeholder.followUp", slash = "/", at = "@")
        } else {
            t!("ui.promptInput.placeholder.normal", slash = "/", at = "@")
        }
    }
    fn composer_shell_mode(&self) -> bool {
        self.add.shell
    }
    fn composer_spellcheck(&self) -> bool {
        self.settings.spellcheck
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
    fn composer_face_bare(&self) -> bool {
        session_view::dock::bare(self)
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
        if crate::popup::key(self, ev, tx)
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
        if crate::keymap::Bind::SubmitAlternate.matches_tx(ev, tx) {
            if session_view::composer::stopping(self) {
                session_view::composer::submit_or_stop(self, tx);
            } else {
                self.submit(true, tx);
            }
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
        t!("common.default").into()
    } else {
        model::capitalize(&model.variant)
    };
    let agent = (s.settings.show_agent || s.subagent_page().is_some()).then(|| model::capitalize(&selected.agent));
    let model_name = s.pickers.model_name(&model);
    let provider = s.pickers.provider_label(&model);
    // What the model's button measures to: the provider, its slash's 6 pt either side, the name.
    let model_label = match &provider {
        Some(provider) => format!("{provider} / {model_name}"),
        None => model_name.clone(),
    };
    let mic = crate::dictation::toolbar(s, p);
    let dictating = s.dictation.active();
    let fold = folds(
        s,
        scope,
        agent.as_deref(),
        &model_label,
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
    out.push(session_view::pickers::view::model_trigger(s, p, provider.as_deref(), &model_name, fold.chevrons).shrink().min_w(0.0));
    if s.has_effort() && fold.effort {
        out.push(session_view::pickers::view::effort_trigger(s, p, &model.variant, fold.chevrons));
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
    // A ghost button: 11 in, then the chevron 4 after the label and 7 from the end, or 11.
    let label = |text: &str, chevron: bool| 11.0 + narrow::text_w(text, UI) + if chevron { 27.0 } else { 11.0 };
    let need = |k: usize| {
        let t = narrow::Toolbar::after(k);
        // The add button and its 2 pt margin, the room, Send, and auto-accept's mark when it's on.
        let mut items = vec![30.0, 0.0, 28.0];
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
        // The slash takes its 6 pt either side, a little more than the spaces measured.
        let whole = label(model, t.chevrons) + if model.contains(" / ") { 4.0 } else { 0.0 };
        items.push(if t.model_whole { whole } else { whole.min(narrow::MODEL_FLOOR) });
        if let Some(effort) = effort.filter(|_| t.effort) {
            items.push(label(effort, t.chevrons));
        }
        items.iter().sum::<f32>() + 2.0 * (items.len() - 1) as f32
    };
    narrow::Toolbar::after(narrow::steps(room, narrow::Toolbar::FOLDS, need))
}

fn agent_icon(s: &App, p: &Pal, name: &str) -> El {
    session_view::pickers::view::agent_icon(s, p, name)
}

pub(super) fn picker_trigger(s: &App, p: &Pal, kind: Kind, label: &str, color: Color, chevron: bool) -> El {
    session_view::pickers::view::trigger(s, p, kind, label, color, chevron)
}

pub(super) fn status_bar(s: &App, p: &Pal, info: Option<&SessionInfo>) -> El {
    let dir = info.map_or(String::new(), |i| s.tilde(&i.directory));
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
