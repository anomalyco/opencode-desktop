//! Desktop host adapter for `opencode-feature-mentions` (`features/mentions`).

pub use opencode_feature_mentions::*;

use std::borrow::Cow;

use opencode_kernel::{Feature, KernelBuilder};
use opencode_kit::client::{Request, Server};
use opencode_kit::mention::UserMention;
use opencode_kit::model::SessionInfo;
use rune::{KeyEvent, PointerPress, TextEdit, Tx, Waker};
use session_view::session_chips::SessionRef;

use crate::{App, Route, Source};

impl MentionsHost for App {
    fn mentions(&self) -> &Mentions {
        &self.mentions
    }

    fn mentions_mut(&mut self) -> &mut Mentions {
        &mut self.mentions
    }

    fn session_refs(&self) -> &refs::Lookups {
        &self.session_refs
    }

    fn session_refs_mut(&mut self) -> &mut refs::Lookups {
        &mut self.session_refs
    }

    fn composer(&self) -> &TextEdit {
        &self.composer
    }

    fn composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn active_session_id(&self) -> Option<&str> {
        App::active_session_id(self)
    }

    fn active_directory(&self) -> String {
        self.composer_directory()
    }

    fn session_info(&self, id: &str) -> Option<&SessionInfo> {
        self.session(id)
    }

    fn sessions(&self) -> &[SessionInfo] {
        &self.sessions
    }

    fn is_session_live(&self, id: &str) -> bool {
        session_view::TimelineHost::is_session_live(self, id)
    }

    fn project_id_at(&self, dir: &str) -> Option<String> {
        self.project_at(dir).map(|p| p.id.clone())
    }

    fn project_label(&self, session: &SessionInfo) -> String {
        App::project_label(self, session)
    }

    fn is_session_tab_open(&self, id: &str) -> bool {
        self.tabs.iter().any(|t| t.id == id)
    }

    fn sessions_stamp(&self) -> SessionsStamp {
        SessionsStamp {
            sessions: (rune::Tracked::id(&self.sessions), rune::Tracked::version(&self.sessions)),
            projects: (rune::Tracked::id(&self.projects), rune::Tracked::version(&self.projects)),
            tabs_version: rune::Tracked::version(&self.tabs),
            tabs_len: self.tabs.len(),
        }
    }

    fn side_tab_keys(&self) -> Option<(Vec<String>, Option<String>)> {
        let region = self.resolve_side_region()?;
        Some((region.strip.iter().map(|e| e.key.clone()).collect(), region.active.clone()))
    }

    fn is_live(&self) -> bool {
        self.live()
    }

    fn live_server(&self) -> Option<Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }
    }

    fn waker(&self) -> Option<Waker> {
        (*self.waker).clone()
    }

    fn send_request(&self, req: Request) {
        if let Source::Live { requests: Some(r), .. } = &*self.source {
            let _ = r.send(req);
        }
    }

    fn push_session_info(&mut self, info: SessionInfo) {
        self.sessions.push(info);
    }

    fn fetch_session_info(&self, id: &str) {
        self.fetch_info(id);
    }

    fn open_session(&mut self, id: &str, tx: &mut Tx) {
        self.open(id, tx);
    }

    fn open_file(&mut self, path: &str, tx: &mut Tx) {
        self.open_workspace_file(path, true, None, tx);
    }

    fn open_context_menu(
        &mut self,
        spec: ContextMenuSpec,
        press: PointerPress,
        tx: &mut Tx,
        on_pick: Box<dyn FnOnce(Option<&str>, &mut Tx) + 'static>,
    ) {
        let mut menu = crate::popup::Menu::new(spec.label).min_width(spec.min_width);
        for item in spec.items {
            menu = menu.item(crate::popup::Item::new(item.id, item.label).icon(item.icon));
        }
        crate::popup::open(self, menu, press, tx, move |_: &mut App, picked: Option<&str>, tx: &mut Tx| {
            on_pick(picked, tx);
        });
    }

    fn auto_focus_composer(&self, tx: &Tx) -> bool {
        crate::auto_focus_composer(tx)
    }

    fn mention_query(&self) -> Option<&str> {
        self.suggest.state.context().map(|l| l.query.as_str())
    }

    fn mention_active(&self) -> Option<&str> {
        self.suggest.state.context().and_then(|l| l.active.id())
    }

    fn refresh_suggestions(&mut self, tx: &mut Tx) {
        App::refresh_suggestions(self, tx);
    }

    fn suggest_pointed(&mut self, key: &str, tx: &mut Tx) {
        App::suggest_pointed(self, key, tx);
    }

    fn suggest_pick(&mut self, key: &str, tx: &mut Tx) {
        App::suggest_pick(self, key, tx);
    }

    fn suggest_search_mut(&mut self) -> &mut TextEdit {
        &mut self.suggest.search
    }

    fn suggest_query(&mut self, tx: &mut Tx) {
        App::suggest_query(self, tx);
    }

    fn suggest_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        App::suggest_key(self, ev, tx)
    }

    fn open_mentions_popover(&mut self, tx: &mut Tx) {
        self.open_context(tx);
    }

    fn dismiss_suggestions(&mut self, tx: &mut Tx) {
        self.suggest(crate::suggestions::machine::Event::Close, tx);
    }
}

impl App {
    /// The composer's directory: the open session's, or where a new one starts.
    pub fn composer_directory(&self) -> String {
        match &*self.route {
            Route::Session { id, .. } => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir().to_owned(),
        }
    }

    /// There's a server to search; the fixture's files and sessions are searched here.
    pub(crate) fn mention_server(&self) -> bool {
        is_server(self)
    }

    /// The open `@` list's query.
    pub fn mention_query(&self) -> Option<String> {
        query(self)
    }

    /// The highlighted row.
    pub fn mention_active(&self) -> Option<String> {
        active(self)
    }

    /// The popover's rows (`contextList`).
    pub fn mention_items(&self) -> Vec<Item> {
        mention_items(self)
    }

    /// Replaces the `@query` before the caret with the picked row's pill.
    pub fn pick_mention(&mut self, key: &str) {
        pick(self, key);
    }

    /// The open list's searches follow its query, and the directory's catalog is read once it opens.
    pub(crate) fn sync_mention_search(&mut self, tx: &mut Tx) {
        sync_search(self, tx);
    }

    /// Takes in what the searches answered and the timers that came due.
    pub fn poll_mentions(&mut self, tx: &mut Tx) -> bool {
        poll(self, tx)
    }

    /// The prompt's text and mentions for what's in the composer.
    pub fn mention_wire(&self) -> Wire {
        mention_wire(self)
    }

    /// A session's title for a mention of it: a loaded session's, or one the `@` search found.
    pub fn mention_title(&self, id: &str) -> Option<String> {
        mention_title(self, id)
    }

    /// Records the composer's mentions for the prompt about to leave as message `id`.
    pub fn echo_mentions(&self, id: &str) {
        echo_mentions(self, id);
    }

    /// Records `Wire::marks` for message `id`, whose echo shows `Wire::text`.
    pub fn echo_wire_mentions(&self, id: &str, marks: Vec<UserMention>) {
        echo_wire_mentions(id, marks);
    }

    /// Opens the session a prompt's badge mentions.
    pub fn open_mentioned(&mut self, id: &str, tx: &mut Tx) {
        open_mentioned(self, id, tx);
    }

    /// Message `id`'s rendered bubble text, borrowing `body` when it holds no `@` mentions.
    pub fn bubble_text<'a>(&self, id: &str, body: &'a str) -> Cow<'a, str> {
        bubble_text(self, id, body)
    }

    /// What session `id` is, as loaded or as the `@` search found it.
    pub fn session_ref_of(&self, id: &str) -> Option<SessionRef> {
        session_ref_of(self, id)
    }

    /// The chip's menu at the pointer: Copy Session ID.
    pub fn open_session_chip_menu(&mut self, id: &str, press: PointerPress, tx: &mut Tx) {
        open_session_chip_menu(self, id, press, tx);
    }
}

/// Kernel feature registration for `@` mentions waker wiring.
pub struct MentionsFeature;

impl Feature<App> for MentionsFeature {
    fn id(&self) -> &'static str {
        "feature.mentions"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_setup("mentions.waker", |s, w| s.mentions.set_waker(w));
    }
}

mod chip_stories {
    //! Timeline › Session chips (scene `session-chips`): a reply naming its subagent's session id
    //! as plain text and in inline code, one the server doesn't have, and the same id in a code
    //! block. The known ids read as the session's title; the rest stay as written. R replays.

    use opencode_kit::model::Message;
    use rune::Play;

    use crate::App;
    use crate::fixture_ids::CHILD;
    use crate::stories::{replay, sid};

    const ASK: &str = "msg_zzzzsessionchips0";
    const REPLY: &str = "msg_zzzzsessionchips1";
    /// A well-formed id no session has.
    const GONE: &str = "ses_fx999900aaaaaaaaaaaaaaaaaa";

    fn words() -> String {
        format!(
            "The review ran in {CHILD} and came back clean; its notes are in `{CHILD}`. The first attempt, {GONE}, was deleted.\n\nTo keep its transcript:\n\n```sh\nopencode export {CHILD}\n```\n"
        )
    }

    /// The open session with Kit's question and the finished reply.
    fn scene(s: &mut App) {
        crate::scenes::open_scene(s);
        s.finish_stream();
        let id = sid(s);
        if let Some(t) = s.transcripts.get_mut(&id) {
            t.messages.push(Message::User { id: ASK.into(), text: "Did the review pass?".into() });
        }
        crate::genui::demo::stream(s, REPLY, &words(), 0.0);
        while let Some((_, ty, data)) = s.playback.pop_front() {
            s.apply_event(&ty, &data);
        }
    }

    fn open(s: &mut App, tx: &mut Play<App>) {
        tx.instant();
        scene(s);
    }

    rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
        crate::stories::Story::session("Timeline/Session chips", &[replay(open)]),
    ]);

    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("session-chips", scene)]);
}
