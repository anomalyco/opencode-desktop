//! The taskbar's jump list (`rune::set_jump_list`): New session and New window, then the recent
//! projects and sessions on the app's own server, each a link on the app's scheme
//! (`deep_links`) that reaches the running app through `single_instance`, or starts it.
//!
//! It costs nothing at launch: the first list goes out from a poll after the window's first
//! frame (and at least two seconds into the run), and later ones only when what they'd show changed, at most every
//! two seconds, on polls the app runs anyway (no timer of its own). Rune commits it on a thread
//! of its own; an unchanged list does nothing there either.

use rune::{JumpItem, JumpList};

use crate::App;

/// How many of each the list shows: Windows shows ten recent entries by default, all categories
/// together.
const RECENT: usize = 5;
/// Seconds between two looks at what the list would show.
const EVERY: f64 = 2.0;

/// `poll`, in a live window: sends the list when it changed.
pub fn poll(s: &mut App) {
    if !s.live() || !s.persists() || !matches!(s.checkout.boot, crate::servers::runtime::Boot::Ready) || *s.clock - s.jump_list_at < EVERY {
        return;
    }
    s.jump_list_at = *s.clock;
    rune::set_jump_list(list(s));
}

fn list(s: &App) -> JumpList {
    let scheme = crate::launch::product().scheme().to_owned();
    let link = |path: &str| format!("{scheme}://{path}");
    let list = JumpList::new()
        .task(JumpItem::new(opencode_i18n::t!("desktop.menu.newSession"), link("new-session")))
        .task(JumpItem::new(opencode_i18n::t!("desktop.menu.newWindow"), link("new-window")));
    // Only the app's own server's: a link opens there.
    if !crate::servers::runtime::active(s).is_sidecar() {
        return list;
    }
    let projects = s
        .used_projects()
        .into_iter()
        .take(RECENT)
        .map(|p| JumpItem::new(if p.name.is_empty() { crate::projects::folder_name(&p.directory) } else { p.name.clone() }, link(&format!("open?project={}", percent(&p.directory)))).description(p.directory.clone()))
        .collect();
    let mut sessions: Vec<_> = s.sessions.iter().filter(|i| i.parent.is_none() && i.archived.is_none() && !i.title.is_empty()).collect();
    sessions.sort_by_key(|i| std::cmp::Reverse(i.updated));
    let sessions = sessions.into_iter().take(RECENT).map(|i| JumpItem::new(i.title.clone(), link(&format!("session?id={}", percent(&i.id)))).description(i.directory.clone())).collect();
    list.category(opencode_i18n::t!("home.projects"), projects).category(opencode_i18n::t!("sidebar.project.recentSessions"), sessions)
}

fn percent(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
