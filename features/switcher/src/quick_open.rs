//! ⌘O, Quick Open (`DialogSessionSearch projects`, `shell/commands/session-search.tsx`): the
//! session search with projects ahead of its sessions. Without a query it lists the projects
//! this client used last; with one, every project the servers know whose name or folder holds
//! it. Choosing a project starts a new session there.

use opencode_i18n::t;
use opencode_kit::model::Project;
use opencode_kit::servers::ServerKey;
use opencode_ui::keymap::Bind;
use rune::{KeyEvent, Tx};

use crate::entries::{Action, Entry, matches};
use crate::{SwitcherHost, SwitcherMode, close, folder_name, open_with_tx, servers};

/// `RECENT_PROJECT_LIMIT`: the projects an empty query lists.
pub const RECENT_PROJECTS: usize = 5;

/// Opens the palette as Quick Open.
pub fn open_quick_open(host: &mut dyn SwitcherHost, tx: &mut Tx) {
    tx.instant();
    open_with_tx(host, SwitcherMode::QuickOpen, tx);
}

/// ⌘O while the palette is open: closes Quick Open, or turns another mode into it. A closed
/// palette leaves the key to the window's `quick.open` command, so the project dialog and the
/// draft's project picker keep it for Add project.
pub(crate) fn key(host: &mut dyn SwitcherHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !host.switcher().is_open() || !Bind::QuickOpen.matches_tx(ev, tx) {
        return false;
    }
    if host.switcher().mode == SwitcherMode::QuickOpen {
        tx.instant();
        close(host, true);
    } else {
        open_quick_open(host, tx);
    }
    true
}

/// The project rows for `q`, every server's: the first few used ones, or each one that matches.
pub(crate) fn projects(host: &dyn SwitcherHost, q: &str) -> Vec<Entry> {
    let typed = !q.is_empty();
    let own = host.active_server();
    let own_tag = servers::own_tag(host);
    let (used, other) = host.listed_projects();
    let mut out: Vec<Entry> = used.into_iter().chain(other.into_iter().filter(|_| typed)).map(|p| entry(&own, own_tag.as_deref(), p)).collect();
    for r in &host.switcher().remotes {
        let listed = r.used.iter().chain(r.other.iter().filter(|_| typed));
        out.extend(listed.map(|p| entry(&r.server, r.tag.as_deref(), p)));
    }
    if typed {
        out.retain(|e| matches(e, q));
    } else {
        out.truncate(RECENT_PROJECTS);
    }
    out
}

/// A project's row: its name and avatar, and its folder, with its server's name once two or more
/// are searched.
fn entry(server: &ServerKey, tag: Option<&str>, p: &Project) -> Entry {
    let title = if p.name.is_empty() { folder_name(&p.directory) } else { p.name.clone() };
    Entry {
        key: format!("project:{server}:{}", p.directory),
        action: Action::Project { server: server.clone(), directory: p.directory.clone() },
        title,
        project: p.id.clone(),
        description: servers::located(&p.directory, tag),
        hint: None,
        chord: None,
        group: t!("palette.group.projects"),
        updated: None,
        open: false,
    }
}
