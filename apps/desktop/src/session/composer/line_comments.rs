//! Desktop host adapter for `opencode-feature-annotations::line_comments` (`features/annotations`).

pub use opencode_feature_annotations::line_comments::*;

use std::rc::Rc;

use opencode_kernel::{Feature, KernelBuilder, Phase};
use opencode_kit::model::CommentOrigin;
use opencode_ui::popup::Menu;
use rune::{El, Tx};
use serde_json::Value;

use crate::App;
use crate::quote::comment::{Config, FieldEvent};

impl LineCommentsHost for App {
    fn line_comments(&self) -> &LineComments {
        &self.line_comments
    }

    fn line_comments_mut(&mut self) -> &mut LineComments {
        &mut self.line_comments
    }

    fn view_selection(&self, spot: &Spot) -> Option<Range> {
        match spot.surface {
            Surface::Files => self
                .files
                .sessions
                .get(&spot.session)?
                .docs
                .get(&spot.path)
                .and_then(host::code_doc_selection),
            Surface::Review | Surface::Narrow => {
                let sr = self.review.sessions.get(&spot.session)?;
                let diffs = if spot.surface == Surface::Review {
                    &sr.diffs
                } else {
                    sr.narrow.diff(&spot.path)?
                };
                host::diff_doc_selection(diffs)
            }
        }
    }

    fn set_view_selection(&mut self, spot: &Spot, range: Option<Range>) {
        match spot.surface {
            Surface::Files => {
                if let Some(d) = self
                    .files
                    .sessions
                    .get_mut(&spot.session)
                    .and_then(|w| w.docs.get_mut(&spot.path))
                {
                    host::set_code_doc_selection(d, range);
                }
            }
            Surface::Review | Surface::Narrow => {
                let Some(sr) = self.review.sessions.get_mut(&spot.session) else { return };
                let diffs = if spot.surface == Surface::Review {
                    Some(&mut sr.diffs)
                } else {
                    sr.narrow.diff_mut(&spot.path)
                };
                if let Some(diffs) = diffs {
                    host::set_diff_doc_selection(diffs, &spot.path, range);
                }
            }
        }
    }

    fn line_preview(&self, spot: &Spot, range: Range) -> Option<String> {
        match spot.surface {
            Surface::Files => {
                let doc = self.files.sessions.get(&spot.session)?.docs.get(&spot.path)?;
                host::code_doc_preview(doc, range)
            }
            Surface::Review | Surface::Narrow => {
                let sr = self.review.sessions.get(&spot.session)?;
                let diffs = if spot.surface == Surface::Review {
                    &sr.diffs
                } else {
                    sr.narrow.diff(&spot.path)?
                };
                host::diff_doc_preview(diffs, &spot.path, range)
            }
        }
    }

    fn showing_surface(&self, sid: &str, path: &str) -> Option<Surface> {
        if self
            .active_file_tab()
            .is_some_and(|(session, p)| session == sid && p.as_deref() == Some(path))
        {
            return Some(Surface::Files);
        }
        let sr = self.review.sessions.get(sid)?;
        if self.narrow() {
            return sr.narrow.is_open(path).then_some(Surface::Narrow);
        }
        (self.is_review_active() && sr.active_path.as_deref() == Some(path))
            .then_some(Surface::Review)
    }

    fn scroll_to_lines(&mut self, spot: &Spot, range: Range, tx: &mut Tx) {
        match spot.surface {
            Surface::Files => {
                if let Some(d) = self
                    .files
                    .sessions
                    .get_mut(&spot.session)
                    .and_then(|w| w.docs.get_mut(&spot.path))
                {
                    host::scroll_code_doc(d, &spot.session, &spot.path, range, tx);
                }
            }
            Surface::Review => {
                if let Some(sr) = self.review.sessions.get_mut(&spot.session) {
                    host::scroll_diff_doc(&mut sr.diffs, &spot.path, range, tx);
                }
            }
            Surface::Narrow => {}
        }
    }

    fn open_comment_source(
        &mut self,
        sid: &str,
        path: &str,
        origin: Option<CommentOrigin>,
        tx: &mut Tx,
    ) {
        match origin {
            Some(CommentOrigin::Review) => {
                self.open_review_for_session(sid, None, None, Some(path), None, tx);
            }
            _ => {
                self.open_workspace_file_for_session(sid, path, false, None, false, tx);
            }
        }
    }

    fn offer_discarded_comment(&mut self, title: &'static str, discarded: Discarded) {
        crate::undo::offer(self, title, crate::undo::Undoable::Comment(discarded));
    }

    fn wrap_comment_menu_button(
        &self,
        trigger: El,
        anchor: String,
        menu: Menu,
        on_pick: Rc<dyn Fn(&mut dyn LineCommentsHost, Option<&str>, &mut Tx)>,
    ) -> El {
        view::popup_menu_button::<App>(trigger, anchor, menu, on_pick)
    }

    fn storage_ready(&self) -> bool {
        crate::storage::is_ready()
    }

    fn load_saved_comments(
        &self,
        namespace: &'static str,
        on_loaded: Box<dyn FnOnce(Vec<(String, String)>) + Send>,
    ) {
        crate::storage::with_storage(move |s| {
            let store = s.clone();
            s.state().load(
                &[namespace],
                Some(Box::new(move || {
                    let rows = store
                        .state()
                        .items(namespace)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(k, v)| (k, v.to_string()))
                        .collect();
                    on_loaded(rows);
                })),
            );
        });
    }

    fn save_comments_json(&self, namespace: &'static str, key: &str, value: &Value) {
        crate::storage::set_json(namespace, key, value);
    }

    fn delete_saved_comments(&self, namespace: &'static str, key: &str) {
        crate::storage::delete(namespace, key);
    }
}

impl App {
    pub fn clear_line_comments(&mut self, sid: &str) {
        clear_line_comments(self, sid);
    }

    pub(crate) fn restore_discarded_comment(&mut self, d: Discarded, tx: &mut Tx) {
        restore_discarded_comment(self, d, tx);
    }

    pub fn open_line_comment_card(&mut self, key: u64, tx: &mut Tx) {
        open_line_comment_card(self, key, tx);
    }

    pub fn remove_line_comment_card(&mut self, key: u64) {
        remove_line_comment_card(self, key);
    }

    pub fn line_comment(&mut self, spot: &Spot, event: machine::Event, tx: &mut Tx) {
        line_comment(self, spot, event, tx);
    }

    pub fn line_comment_field(&mut self, spot: &Spot, event: FieldEvent, tx: &mut Tx) -> bool {
        line_comment_field(self, spot, event, tx)
    }

    pub fn line_comment_config(&self, spot: &Spot) -> Config {
        line_comment_config(self, spot)
    }

    pub fn remove_line_comment(&mut self, sid: &str, id: &str) {
        remove_line_comment(self, sid, id);
    }
}

/// Kernel feature registration for timeline/terminal quoting and code/diff line comments.
pub struct AnnotationsFeature;

impl Feature<App> for AnnotationsFeature {
    fn id(&self) -> &'static str {
        "feature.annotations"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_setup("annotations.waker", |s, w| {
            crate::quote::setup(s, w.clone());
            setup(s, w);
        });
        builder.add_system(Phase::Derive, "quote.poll", |s, now, tx| {
            crate::quote::poll(s, now, tx)
        });
        builder.add_system(Phase::Derive, "line_comments.poll", |s, _, tx| poll(s, tx));
    }
}
