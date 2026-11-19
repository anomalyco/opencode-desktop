//! Desktop host adapter for `opencode-feature-find` (`features/find`).

pub use opencode_feature_find::*;

use std::borrow::Cow;

use opencode_kernel::{CommandEntry, EscapeLayer, Feature, KernelBuilder, Phase};
use opencode_kit::model::Transcript;
use rune::{KeyEvent, Tx};
use session_view::disclosure::Disclosures;

use crate::keymap::Bind;
use crate::pickers::PickerHost;
use crate::{App, Route};

impl FindHost for App {
    fn find(&self) -> &Find {
        &self.timeline_fx.find
    }

    fn find_mut(&mut self) -> &mut Find {
        &mut self.timeline_fx.find
    }

    fn is_home(&self) -> bool {
        matches!(*self.route, Route::Home)
    }

    fn transcript(&self, session: &str) -> Option<&Transcript> {
        self.transcripts.get(session)
    }

    fn transcript_version(&self, _session: &str) -> u64 {
        rune::Tracked::version(&self.transcripts)
    }

    fn session_revert(&self, session: &str) -> Option<&str> {
        self.session(session).and_then(|i| i.revert.as_deref())
    }

    fn is_session_stopped(&self, session: &str) -> bool {
        self.stops.has(session)
    }

    fn code_block_lang(&self, language: &str) -> bool {
        crate::plugin_host::surfaces::code_block_lang(self, language)
    }

    fn bubble_text<'a>(&self, id: &str, body: &'a str) -> Cow<'a, str> {
        App::bubble_text(self, id, body)
    }

    fn expanded(&self) -> &Disclosures {
        &self.timeline_fx.expanded
    }

    fn expanded_mut(&mut self) -> &mut Disclosures {
        &mut self.timeline_fx.expanded
    }

    fn fold_user_messages(&self) -> bool {
        self.settings.fold_user
    }

    fn composer_focused_mut(&mut self) -> &mut bool {
        &mut self.composer.focused
    }

    fn visible_timeline_rows(&self, session: &str) -> Vec<String> {
        self.timeline_fx.ends.recall.marks(session).visible()
    }

    fn timeline_view_rect(&self) -> Option<[f32; 4]> {
        self.timeline_fx.message_nav.view
    }

    fn timeline_page_rect(&self) -> Option<[f32; 4]> {
        self.timeline_fx.message_nav.page
    }

    fn is_narrow(&self) -> bool {
        self.narrow()
    }

    fn has_unloaded_older_messages(&self, session: &str) -> bool {
        !self.pages.first_read_done(session) || self.pages.more_above(session)
    }

    fn read_older_messages(&mut self, session: &str, tx: &mut Tx) {
        self.read_older(session, tx);
    }

    fn settings_open(&self) -> bool {
        self.settings.open
    }

    fn session_opening(&self) -> bool {
        self.opening.is_some()
    }

    fn close_picker(&mut self) {
        PickerHost::close_picker(self, false);
    }

    fn focus_home_search(&mut self, tx: &mut Tx) {
        App::focus_home_search(self, tx);
    }

    fn handle_input_fallback_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.switcher_key(ev, tx) || self.shortcut(ev, tx)
    }
}

impl App {
    pub fn find_focused(&self) -> bool {
        is_focused(self)
    }

    pub fn open_find(&mut self, tx: &mut Tx) {
        open(self, tx);
    }

    pub fn open_find_at(&mut self, query: &str, message: &str, tx: &mut Tx) {
        open_at(self, query, message, tx);
    }

    pub fn close_find(&mut self, tx: &mut Tx) {
        close(self, tx);
    }

    pub fn step_find(&mut self, delta: i32, tx: &mut Tx) {
        step(self, delta, tx);
    }

    #[allow(dead_code)]
    pub(crate) fn sync_find_index(&mut self) {
        sync_index(self);
    }
}

/// Kernel feature registration for session timeline Find.
pub struct FindFeature;

impl Feature<App> for FindFeature {
    fn id(&self) -> &'static str {
        "feature.find"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_command(
            CommandEntry::<App>::from_key(
                "find",
                opencode_i18n::key!("rune.command.session.find"),
                crate::commands::order::FIND,
                |s, tx| s.open_find(tx),
            )
            .bind_id(Bind::Find.id())
            .when(|s| s.active_session_id().is_some()),
        );
        builder.register_escape_layer(EscapeLayer::new(
            "escape.find",
            40,
            |s| s.timeline_fx.find.is_open() && matches!(*s.route, Route::Session { .. }),
            |s, ev, tx| {
                dismiss_key(s, ev, tx);
            },
        ));
        builder.add_setup("find.waker", |s, w| setup(s, w));
        builder.add_system(Phase::Derive, "find.poll", |s, _, tx| poll(s, tx));
    }
}
