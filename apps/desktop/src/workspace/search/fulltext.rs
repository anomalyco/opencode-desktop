//! Desktop host adapter for `opencode-feature-workspace-tools::fulltext`
//! (`features/workspace-tools`).

pub use opencode_feature_workspace_tools::fulltext::*;

use std::sync::Arc;

use opencode_kit::model::{Project, SessionInfo};
use opencode_storage::Storage;

use crate::App;

impl FulltextHost for App {
    fn fulltext(&self) -> &Surfaces {
        &self.fulltext
    }

    fn fulltext_mut(&mut self) -> &mut Surfaces {
        &mut self.fulltext
    }

    fn surface_query(&self, which: Surface) -> &str {
        match which {
            Surface::Palette if self.switcher.open && self.switcher.mode != crate::switcher::SwitcherMode::Commands => {
                self.switcher.query()
            }
            Surface::Home if self.home_search_open() => self.search.text.as_str(),
            _ => "",
        }
    }

    fn active_server_key(&self) -> &str {
        crate::servers::runtime::active(self).as_str()
    }

    fn palette_archived(&self) -> bool {
        self.switcher.mode == crate::switcher::SwitcherMode::Archived
    }

    fn home_selected_id(&self) -> Option<String> {
        App::home_selected_id(self)
    }

    fn root_of<'a>(&'a self, id: &'a str) -> &'a str {
        App::root_of(self, id)
    }

    fn session(&self, id: &str) -> Option<&SessionInfo> {
        App::session(self, id)
    }

    fn search_session(&self, id: &str) -> Option<&SessionInfo> {
        App::search_session(self, id)
    }

    fn project_for_session(&self, session: &SessionInfo) -> Option<&Project> {
        App::project_for_session(self, session)
    }

    fn tab_title_and_dir(&self, session_id: &str) -> Option<(&str, &str)> {
        self.tabs
            .iter()
            .find(|t| t.id == session_id && !t.title.is_empty())
            .map(|t| (t.title.as_str(), t.directory.as_str()))
    }

    fn with_storage(&self, op: Box<dyn FnOnce(&Storage) + Send>) {
        crate::storage::with_storage(op);
    }
}

impl App {
    pub(crate) fn indexing_history(&self, which: Surface) -> bool {
        opencode_feature_workspace_tools::fulltext::indexing_history(self, which)
    }

    pub(crate) fn poll_fulltext(&mut self) -> bool {
        opencode_feature_workspace_tools::fulltext::poll_fulltext(self)
    }

    pub(crate) fn fulltext_rows(&self, which: Surface) -> &[Row] {
        opencode_feature_workspace_tools::fulltext::fulltext_rows(self, which)
    }

    pub(crate) fn fulltext_row(&self, which: Surface, id: &str) -> Option<(Arc<Vec<Row>>, usize)> {
        opencode_feature_workspace_tools::fulltext::fulltext_row(self, which, id)
    }
}
