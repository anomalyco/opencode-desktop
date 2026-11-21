//! Where Ctrl+N and New session open a draft (Settings › New sessions start in): the project on
//! screen in its default worktree, a new worktree or wherever its last session started, or the
//! folder the server was started in. A server's folder is its default location
//! (`GET /api/location` with no directory, `packages/server/src/location.ts`), read when its
//! stream opens and again after it comes back.

use std::collections::HashMap;

use opencode_kit::client::Request;
use opencode_kit::servers::ServerKey;

use crate::settings::{StartIn, Worktree};
use crate::{App, Source};

/// Each server's default directory, once it has said.
#[derive(Default)]
pub struct ServerDirectories(HashMap<ServerKey, String>);

pub(crate) static SERVER_DIRECTORIES: opencode_kernel::Slot<ServerDirectories> = opencode_kernel::Slot::new();

/// Asks the server the turn works against for its default directory.
pub(crate) fn ask(s: &App) {
    if let Source::Live { requests: Some(r), .. } = &*s.source {
        r.send(Request::DefaultDirectory).ok();
    }
}

pub(crate) fn answered(s: &mut App, directory: String) {
    if directory.is_empty() {
        return;
    }
    s.kernel.slots.get_mut(&SERVER_DIRECTORIES).0.insert(crate::servers::runtime::current(), directory.clone());
    if s.projects.is_empty() && crate::arg("--directory").is_none() && *s.default_dir == *s.home {
        let home = (*s.home).clone();
        *s.default_dir = directory.clone();
        let mut updated = false;
        for tab in s.tabs.iter_mut() {
            if tab.is_draft() && tab.directory == home {
                tab.directory.clone_from(&directory);
                updated = true;
            }
        }
        if updated {
            s.save_tabs();
        }
    }
    if s.composer_ext.pickers.models.is_empty() {
        crate::connect::picker_reads::refresh(s);
    }
}

/// The default directory of the server the turn works against.
pub(crate) fn server_directory(s: &App) -> Option<&str> {
    s.kernel.slots.get(&SERVER_DIRECTORIES).0.get(&crate::servers::runtime::current()).map(String::as_str)
}

/// The folder a new draft opens in, given the one the page is on (`here`): the server's default
/// directory, or the page's project in its main checkout unless the draft goes to a new or last
/// used worktree (`App::apply_default_destination`). Before the server has said where it
/// started, Default directory falls back to the page's folder and asks.
pub(crate) fn resolve(s: &App, here: Option<String>) -> Option<String> {
    match s.settings.start_in {
        StartIn::Directory => match server_directory(s) {
            Some(dir) => Some(dir.to_owned()),
            None => {
                ask(s);
                here
            }
        },
        StartIn::Project => {
            let picked = match s.settings.worktree {
                Worktree::Default => here.map(|dir| s.project_at(&dir).map_or(dir, |p| p.directory.clone())),
                Worktree::New | Worktree::LastUsed => here,
            };
            picked.or_else(|| match server_directory(s) {
                Some(dir) => Some(dir.to_owned()),
                None => {
                    ask(s);
                    None
                }
            })
        }
    }
}
