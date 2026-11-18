//! Auto-accept permissions, as in Desktop (`session/requests/auto-approve.ts`, the `permissions.autoaccept`
//! command on ⇧⌘A, Settings › General). One client-local switch covers every session and project: while
//! it's on, each permission request is answered "once" the moment it's known, pending ones included,
//! and the permission card stays hidden. Forms and questions still wait for a person.

use opencode_kit::model::Pending;
use rune::Tx;

/// Object-safe host boundary for permission auto-accept settings, toggle commands, and
/// immediate permission answering.
pub trait AutoAcceptHost {
    /// Global default auto-accept preference.
    fn auto_accept_default(&self) -> bool;
    /// Project-specific auto-accept override for `project_id`, if set.
    fn project_auto_accept(&self, project_id: &str) -> Option<bool>;
    /// Persists a change to the global default auto-accept preference.
    fn set_auto_accept_default(&mut self, on: bool);
    /// Persists a project-specific auto-accept override (`None` resets to the global default).
    fn set_project_auto_accept_override(&mut self, project_id: &str, on: Option<bool>);

    /// Currently active session ID on the open route, if any.
    fn active_session_id(&self) -> Option<&str>;
    /// Project ID owning `session_id`, if resolved.
    fn project_id_for_session(&self, session_id: &str) -> Option<&str>;

    /// Pending permission and form requests across sessions.
    fn pending(&self) -> &[Pending];
    /// Answers permission request `id` with `decision` (e.g. `"once"`).
    fn reply_permission(&mut self, id: &str, decision: &str);
    /// Displays an informational status toast.
    fn toast_say(&mut self, title: String, description: String);
}

/// Sets the global default auto-accept preference and answers newly covered pending permissions.
pub fn set_auto_accept(host: &mut dyn AutoAcceptHost, on: bool) {
    host.set_auto_accept_default(on);
    answer_auto_accepted(host);
}

/// A project's own Auto-accept permissions: on, off, or `None` for the default.
pub fn set_project_auto_accept(host: &mut dyn AutoAcceptHost, project: &str, on: Option<bool>) {
    host.set_project_auto_accept_override(project, on);
    answer_auto_accepted(host);
}

/// Whether session `session`'s permission requests are approved without asking: its
/// project's own setting, else the default (`permissions.autoApproves`).
pub fn auto_accepts(host: &dyn AutoAcceptHost, session: &str) -> bool {
    host.project_id_for_session(session)
        .and_then(|id| host.project_auto_accept(id))
        .unwrap_or_else(|| host.auto_accept_default())
}

/// The open session's project, which ⇧⌘A and the command act on (`sessionProject`).
pub fn auto_accept_project(host: &dyn AutoAcceptHost) -> Option<String> {
    let id = host.active_session_id()?;
    host.project_id_for_session(id).map(str::to_owned)
}

/// Whether auto-accept is on where ⇧⌘A acts (`autoAccepting`).
pub fn auto_accepting(host: &dyn AutoAcceptHost) -> bool {
    match auto_accept_project(host) {
        Some(p) => host.project_auto_accept(&p).unwrap_or_else(|| host.auto_accept_default()),
        None => host.auto_accept_default(),
    }
}

/// ⇧⌘A (`toggleAutoAccept`, `use-session-commands.tsx:439-467`): from a session it sets the
/// project's own choice, back to the default when they match; elsewhere the default. A toast
/// says what it did.
pub fn toggle_auto_accept(host: &mut dyn AutoAcceptHost, tx: &mut Tx) {
    use opencode_i18n::t;
    tx.instant();
    let project = auto_accept_project(host);
    let active = !auto_accepting(host);
    let default = host.auto_accept_default();
    match &project {
        None => set_auto_accept(host, active),
        Some(p) => set_project_auto_accept(host, p, (active != default).then_some(active)),
    }
    let (title, description) = match (active, project.is_some()) {
        (false, _) => (t!("toast.permissions.autoaccept.off.title"), t!("toast.permissions.autoaccept.off.description")),
        (true, true) => (t!("toast.permissions.autoaccept.project.title"), t!("toast.permissions.autoaccept.project.description")),
        (true, false) => (t!("toast.permissions.autoaccept.on.title"), t!("toast.permissions.autoaccept.on.description")),
    };
    host.toast_say(title.to_owned(), description.to_owned());
}

/// A request that just arrived: answered at once when auto-accept is on.
pub fn auto_answer(host: &mut dyn AutoAcceptHost, id: &str) {
    let ro: &dyn AutoAcceptHost = &*host;
    let covered = ro.pending().iter().any(|p| {
        matches!(p, Pending::Permission { id: pid, session, .. } if pid == id && auto_accepts(ro, session))
    });
    if covered {
        host.reply_permission(id, "once");
    }
}

/// Answers the waiting permission requests auto-accept now covers.
fn answer_auto_accepted(host: &mut dyn AutoAcceptHost) {
    let ro: &dyn AutoAcceptHost = &*host;
    let open: Vec<String> = ro
        .pending()
        .iter()
        .filter_map(|p| match p {
            Pending::Permission { id, session, .. } if auto_accepts(ro, session) => Some(id.clone()),
            _ => None,
        })
        .collect();
    for id in open {
        host.reply_permission(&id, "once");
    }
}
