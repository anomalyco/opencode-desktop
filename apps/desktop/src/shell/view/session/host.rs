//! The Desktop window as a host for the shared session timeline.

use opencode_kit::client::Request;
use opencode_kit::model::{Inbox, Message, Transcript};
use rune::Tx;

use crate::pickers::PickerHost as _;
use crate::{
    App, Route, Source, agent_rows, background, copied, diagram, disclosure, jump, line_reveal,
    narrow, palette, pickers, plugin_host, popup, queue, quote, rewind, send_failed, settings,
    speech, subagents, tool_open, working,
};

impl session_view::TimelineHost for App {
    fn knobs(&self) -> &rune_knobs::Knobs {
        &self.diag.knobs
    }
    fn assistive(&self) -> bool {
        speech::listening(self)
    }
    fn clock(&self) -> f64 {
        *self.clock
    }
    fn now_ms(&self) -> i64 {
        *self.now_ms
    }
    fn reduced_motion(&self) -> bool {
        *self.appearance_state.reduced_motion
    }
    fn smooth_timeline(&self) -> bool {
        self.settings.smooth_timeline && !*self.appearance_state.reduced_motion
    }
    fn timeline_detail(&self) -> session_view::detail::Detail {
        self.settings.timeline_detail
    }
    fn code_block_lang(&self, language: &str) -> bool {
        plugin_host::surfaces::code_block_lang(self, language)
    }
    fn code_block(&self, p: &palette::Pal, language: &str, key: &str, code: &str, streaming: bool, version: u64, native: rune::El) -> Option<rune::El> {
        plugin_host::surfaces::code_block(self, p, language, key, code, streaming, version, native)
    }
    fn shell_output(&self, shell: &str) -> Option<&str> {
        self.shells.follow.output(shell)
    }

    fn output_save_failed(&mut self, reason: String) {
        crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), reason);
    }
    fn output_save_reporter(&mut self) -> Box<dyn FnOnce(String) + Send> {
        let (send, _) = self.kernel.slots.slot::<crate::app::OutputErrors>().0.get_or_insert_with(std::sync::mpsc::channel);
        let (send, wake) = (send.clone(), self.waker.as_ref().cloned());
        Box::new(move |why| { if send.send(why).is_ok() && let Some(wake) = wake { wake(); } })
    }
    fn shell_output_version(&self) -> u64 {
        self.shells.follow.version()
    }
    fn turn_diffs(&self) -> bool {
        self.settings.turn_diffs
    }
    fn turn_diff(&self, session: &str, from: &str) -> session_view::turn_diff::State<'_> {
        self.turn_diffs.view(session, from)
    }
    fn retry_turn_diff(&mut self, session: &str, from: &str) {
        self.turn_diffs.retry(session, from);
    }
    fn lane_align(&self) -> session_view::jump::Align {
        match self.settings.background_activity {
            settings::BackgroundActivity::Center => session_view::jump::Align::Center,
            settings::BackgroundActivity::Right => session_view::jump::Align::End,
            settings::BackgroundActivity::Left | settings::BackgroundActivity::Off => session_view::jump::Align::Start,
        }
    }
    fn content_width(&self) -> f32 {
        narrow::content_width(self).unwrap_or_else(|| self.settings.timeline_width.width().content())
    }
    fn timeline_frame(&self) -> Option<&'static str> {
        Some("session")
    }
    fn active_session_id(&self) -> Option<&str> {
        match &*self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }
    fn transcript(&self, session: &str) -> Option<&Transcript> {
        rune::Tracked::untracked(&self.transcripts).get(session)
    }
    fn transcript_version(&self, session: &str) -> Option<u64> {
        Some(self.family_transcript_version(session).1)
    }
    fn transcript_changed_from(&self, session: &str, since: u64) -> Option<usize> {
        App::transcript_changed_from(self, session, since)
    }
    fn session_directory(&self, session: &str) -> Option<&str> {
        self.session(session).map(|i| i.directory.as_str())
    }
    fn session_title(&self, session: &str) -> Option<String> {
        self.mention_title(session)
    }
    fn is_session_live(&self, session: &str) -> bool {
        self.running.contains(session)
            || (rune::Tracked::untracked(&self.transcripts).get(session).is_some_and(|t| self.timeline_busy(t)) && !self.stops.has(session))
    }
    fn timeline_busy(&self,t:&Transcript)->bool {
        let last=t.messages.iter().rev().find(|m| !matches!(m,Message::Notice(_)) && !matches!(self.unsent.state(m.id()),Some(send_failed::State::Waiting|send_failed::State::Unconfirmed|send_failed::State::Failed)));
        matches!(last,Some(Message::User{..})) || matches!(last,Some(Message::Assistant(a)) if a.completed.is_none() && a.error.is_none())
    }
    fn working_label_for(&self, session: &str) -> Option<&'static str> {
        self.queue.starts.status(session)
    }
    fn is_session_stopped(&self, session: &str) -> bool {
        self.stops.has(session)
    }
    fn session_waiting(&self, session: &str) -> Option<session_view::working::Waiting> {
        self.pending.iter().find(|p| p.session() == session).map(|p| match p {
            opencode_kit::model::Pending::Permission { .. } => session_view::working::Waiting::Approval,
            opencode_kit::model::Pending::Form { .. } => session_view::working::Waiting::Answer,
        })
    }
    fn session_revert(&self, session: &str) -> Option<&str> {
        self.session(session).and_then(|i| i.revert.as_deref())
    }
    fn for_each_tracked_session(&self, mut f: impl FnMut(&str)) {
        for id in self.transcripts.keys().chain(self.running.iter()) {
            f(id);
        }
    }
    fn expanded(&self) -> &disclosure::Disclosures {
        &self.timeline_fx.expanded
    }
    fn expanded_mut(&mut self) -> &mut disclosure::Disclosures {
        &mut self.timeline_fx.expanded
    }
    fn copied_state(&self) -> &copied::Copied {
        &self.timeline_fx.copied
    }
    fn copied_state_mut(&mut self) -> &mut copied::Copied {
        &mut self.timeline_fx.copied
    }
    fn working_state(&self) -> &working::Working {
        &self.timeline_fx.working
    }
    fn working_state_mut(&mut self) -> &mut working::Working {
        &mut self.timeline_fx.working
    }
    fn diagrams(&self) -> &diagram::Diagrams {
        &self.timeline_fx.diagram
    }
    fn diagrams_mut(&mut self) -> &mut diagram::Diagrams {
        &mut self.timeline_fx.diagram
    }
    fn media_store(&self) -> &session_view::images::MediaStore {
        &self.composer_ext.attach.store
    }
    fn media_store_mut(&mut self) -> &mut session_view::images::MediaStore {
        &mut self.composer_ext.attach.store
    }
    fn line_hold(&self) -> &line_reveal::Hold {
        &self.timeline_fx.line_hold
    }
    fn line_hold_mut(&mut self) -> &mut line_reveal::Hold {
        &mut self.timeline_fx.line_hold
    }
    fn ends(&self) -> &jump::Ends {
        &self.timeline_fx.ends
    }
    fn ends_mut(&mut self) -> &mut jump::Ends {
        &mut self.timeline_fx.ends
    }
    fn fold_user_messages(&self) -> bool {
        self.settings.fold_user
    }
    fn open_link(&mut self, url: &str, tx: &mut Tx) {
        self.open_reply_link(url, tx);
    }
    fn context_menu(&mut self, at: session_view::images::menu::MenuAt, press: rune::PointerPress, tx: &mut Tx) {
        popup::context::open(self, at, press, tx);
    }
    fn code_links(&self, body: &str, live: bool) -> Option<std::rc::Rc<std::collections::HashSet<String>>> {
        self.reply_code_links(body, live)
    }
    fn code_links_version(&self) -> u64 {
        self.active_session_id().map_or(0, |sid| rune::Tracked::untracked(&self.workspace_panes.files).reply_version(sid))
    }
    fn reply_link_help(url: &str) -> Option<String> {
        crate::files::link_tip::tip(url, "")
    }
    fn code_block_actions(p: &palette::Pal, language: Option<&str>, source: &str) -> Vec<rune::El> {
        plugin_host::code_block_actions(p, language, source)
    }
    fn send_request(&self, req: Request) {
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            let _ = r.send(req);
        }
    }
    fn can_switch_model(&self) -> bool {
        true
    }
    fn switch_model(&mut self, _tx: &mut Tx) {
        if self.composer_ext.pickers.open() != Some(pickers::Kind::Model) {
            self.open_picker(pickers::Kind::Model);
        }
    }
    fn model_name(&self, model: &opencode_kit::model::ModelRef) -> String {
        self.composer_ext.pickers.model_name(model)
    }
    fn tool_row_action(&self, p: &palette::Pal, session: &str, call: &session_view::tools::Call) -> Option<rune::El> {
        tool_open::action(self, p, session, call)
    }
    fn tool_row_action_version(&self, id: &str) -> u64 {
        u64::from(self.tool_step_showing(id))
    }
    fn can_open_subagent(&self) -> bool {
        true
    }
    fn resolve_subagent<'a>(&'a self, parent: &str, session: Option<&'a str>, title: &str) -> Option<&'a str> {
        subagents::child_of(self, parent, session, title)
    }
    fn subagent_run(&self, parent: &str, call: &str, child: Option<&str>, status: &opencode_kit::model::ToolStatus) -> agent_rows::Run {
        self.agent_run(parent, call, child, status)
    }
    fn prefetch_subagent(&self, child: &str) {
        self.prefetch_session(child);
    }
    fn subagent_hover(&self) -> Option<&str> {
        self.timeline_fx.agent_hover.as_deref()
    }
    fn set_subagent_hover(&mut self, call: Option<String>) {
        *self.timeline_fx.agent_hover = call;
    }
    fn open_subagent(&mut self, parent: &str, child: &str, title: &str, agent: &str, tx: &mut Tx) {
        self.open_child(parent, child, title, agent, tx);
    }
    fn can_open_session(&self) -> bool {
        true
    }
    fn open_session(&mut self, id: &str, tx: &mut Tx) {
        self.open_mentioned(id, tx);
    }
    fn session_ref(&self, id: &str) -> Option<session_view::session_chips::SessionRef> {
        self.session_ref_of(id)
    }
    fn session_chip_menu(&mut self, id: &str, press: rune::PointerPress, tx: &mut Tx) {
        self.open_session_chip_menu(id, press, tx);
    }
    fn loading_error(&self, session: &str) -> Option<&str> {
        self.opening.as_ref().filter(|o| o.id == session).and_then(|o| o.error())
    }
    fn partial_output(&self, session: &str) -> Option<&str> {
        self.opening.as_ref().filter(|o| o.id == session).and_then(|o| o.partial())
    }
    fn loading_older(&self, session: &str) -> bool {
        self.pages.loading_older(session)
    }
    fn more_above(&self, session: &str) -> bool {
        self.pages.more_above(session)
    }
    fn watch_scroll_top(&self, session: &str) -> Option<Box<dyn Fn(rune::ScrollPos)>> {
        let (pages, nav) = (self.pages.watch_top(session), self.timeline_fx.message_nav.watch());
        Some(Box::new(move |at| {
            pages(at);
            nav(at);
        }))
    }
    fn on_timeline_selection_change(s: &mut Self, change: &rune::SelectionChange, tx: &mut Tx) {
        quote::selection_changed(s, change, tx);
    }
    fn find_row_version(&self, row_key: &str) -> u64 {
        self.timeline_fx.find.row_version(row_key)
    }
    fn find_text_highlights(&self, p: &palette::Pal, row_key: &str) -> Vec<rune::TextHighlight> {
        self.timeline_fx.find.text_highlights(p, row_key)
    }
    fn find_markdown_highlights(&self, p: &palette::Pal, row_key: &str) -> Vec<(&[std::ops::Range<usize>], rune_kit::Highlight)> {
        self.timeline_fx.find.markdown_highlights(p, row_key)
    }
    fn find_row_hits(&self, row_key: &str) -> Option<(&[Vec<std::ops::Range<usize>>], Option<usize>)> {
        self.timeline_fx.find.row_hits(row_key)
    }
    fn user_menu_open_on(&self, id: &str) -> bool {
        self.dialogs.rewind.menu_open_on(id)
    }
    fn quote_more_open(&self, id: &str) -> bool {
        self.composer_ext.quote.more.contains(id)
    }
    fn open_quote_more(&mut self, id: String) {
        self.composer_ext.quote.more.insert(id);
    }
    fn user_menu(&self, p: &palette::Pal, session: &str, id: &str, _body: &str, pending: bool) -> Option<rune::El> {
        rewind::user_menu(self, p, session, id, pending)
    }
    fn user_status(&self, p: &palette::Pal, id: &str) -> Option<rune::El> {
        send_failed::footer(self, p, id)
    }
    fn user_status_version(&self, id: &str) -> u64 {
        send_failed::footer_version(self, id)
    }
    fn inbox_for(&self, session: &str) -> Option<&Inbox> {
        App::inbox_for(self, session)
    }
    fn queued_compaction(&self, session: &str) -> Option<&str> {
        self.inboxes.get(session).and_then(|i| i.compaction.as_deref())
    }
    fn pending_steer_label(&self, item: &opencode_kit::model::InboxItem) -> String {
        queue::pending_label(item)
    }
    fn turn_tail_extra(&self, p: &palette::Pal, session: &str, busy: bool) -> Option<rune::El> {
        background::hint(self, p, session, busy)
    }
}
