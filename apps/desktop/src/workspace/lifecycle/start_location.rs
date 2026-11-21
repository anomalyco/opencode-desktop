//! Where Ctrl+N and New session open a draft (Settings › New sessions start in): the project on
//! screen in its default worktree, a new worktree or wherever its last session started, or the
//! folder the server was started in. A server's folder is its default location
//! (`GET /api/location` with no directory, `packages/server/src/location.ts`), read when its
//! stream opens and again after it comes back.

use std::collections::{HashMap, HashSet};

use opencode_kit::client::Request;
use opencode_kit::servers::ServerKey;

use crate::settings::{StartIn, Worktree};
use crate::{App, Source};

/// Each server's default directory, once it has said, plus which servers have already triggered
/// the one-shot empty-model catalog refresh on initial `DefaultDirectory`.
#[derive(Default)]
pub struct ServerDirectories {
    dirs: HashMap<ServerKey, String>,
    refreshed_empty: HashSet<ServerKey>,
}

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
    let server = crate::servers::runtime::current();
    let prev = s.kernel.slots.get_mut(&SERVER_DIRECTORIES).dirs.insert(server.clone(), directory.clone());
    // Only backfill drafts on this server that defaulted to `$HOME` before this server's first
    // `DefaultDirectory` reply arrived; never rewrite another server's drafts or an intentional
    // `$HOME` draft picked after initial connect.
    if prev.is_none() && s.projects.is_empty() && crate::arg("--directory").is_none() {
        let home = (*s.home).clone();
        if *s.default_dir == home && !s.tabs.iter().any(|t| t.server != server) {
            *s.default_dir = directory.clone();
        }
        let mut updated = false;
        for tab in s.tabs.iter_mut() {
            if tab.server == server && tab.is_draft() && tab.directory == home {
                tab.directory.clone_from(&directory);
                updated = true;
            }
        }
        if updated {
            s.save_tabs();
        }
    }
    // Retry empty model catalogs at most once per server on initial `DefaultDirectory` so a server
    // with zero configured models does not fire four catalog GETs on every reconnect.
    if s.composer_ext.pickers.models.is_empty()
        && s.kernel.slots.get_mut(&SERVER_DIRECTORIES).refreshed_empty.insert(server)
    {
        crate::connect::picker_reads::refresh(s);
    }
}

/// The default directory of the server the turn works against.
pub(crate) fn server_directory(s: &App) -> Option<&str> {
    s.kernel.slots.get(&SERVER_DIRECTORIES).dirs.get(&crate::servers::runtime::current()).map(String::as_str)
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
