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

/// When the jump list was last looked at (stored in `App::kernel.slots`).
#[derive(Default)]
struct JumpListClock(f64);

static JUMP_LIST_CLOCK: opencode_kernel::Slot<JumpListClock> = opencode_kernel::Slot::new();

/// `poll`, in a live window: sends the list when it changed.
pub fn poll(s: &mut App) {
    let last = s.kernel.slots.try_untracked(&JUMP_LIST_CLOCK).map_or(0.0, |c| c.0);
    if !s.live() || !s.persists() || !matches!(s.checkout.boot, crate::servers::runtime::Boot::Ready) || *s.clock - last < EVERY {
        return;
    }
    s.kernel.slots.get_or_default(&JUMP_LIST_CLOCK).0 = *s.clock;
    // A run under a scratch root (`RUNE_DIRS_ROOT`) shares the installed app's identity unless it
    // took its own (`--app-id`), and leaves that list alone. Under its own, a pick carries the
    // root back (`--dirs-root`, which only debug builds read) so it reaches this run, not the
    // usual profile; a release build's scratch run has no list.
    if !crate::dirs::rooted() {
        return rune::set_jump_list(list(s));
    }
    if let Some(lead) = scratch_lead() {
        rune::set_jump_list(list(s).lead(lead));
    }
}

/// What brings a scratch run's picks back to it, when they can come back: it has an identity of
/// its own and is a debug build.
fn scratch_lead() -> Option<String> {
    let own_identity = rune::extra_args().iter().any(|a| a.starts_with("--app-id="));
    // Debug-only harness jump-list helper reads RUNE_DIRS_ROOT (set at startup via --dirs-root) to forward it to jump-list tasks.
    #[allow(clippy::disallowed_methods)]
    let root = std::env::var("RUNE_DIRS_ROOT").ok()?;
    (own_identity && cfg!(debug_assertions)).then(|| format!("--dirs-root={root}"))
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
    sessions.sort_by_key(|i| std::cmp::Reverse(s.recent_page.active_ms(i)));
    let sessions = sessions.into_iter().take(RECENT).map(|i| JumpItem::new(i.title.clone(), link(&format!("session?id={}", percent(&i.id)))).description(i.directory.clone())).collect();
    list.category(opencode_i18n::t!("home.projects"), projects).category(opencode_i18n::t!("sidebar.project.recentSessions"), sessions)
}

fn percent(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
