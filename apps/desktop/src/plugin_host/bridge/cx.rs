//! How a binding reaches the app. Every entry into a plugin's VM happens inside [`enter`]:
//! renders and queries read the window's `App` through [`with_app`]; what a binding wants changed
//! (open a panel, show a toast) it queues with [`ask`], and the host carries the queue out with
//! `&mut App` and the transaction as soon as the entry returns ([`super::requests::carry_out`]).
//! A render also records what it read ([`read`]), so the host knows when to run it again.
// The app pointer a binding reads is set and cleared around each entry; each block says why.
#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::App;

thread_local! {
    static APP: Cell<*const App> = const { Cell::new(std::ptr::null()) };
    static READS: RefCell<Option<Vec<Dep>>> = const { RefCell::new(None) };
    static PLUGIN: RefCell<Option<Rc<str>>> = const { RefCell::new(None) };
}

/// Runs `f` as plugin `plugin`'s render: nodes that act for their plugin (`ui.flyoutRow`)
/// read it with [`rendering`].
pub fn rendering_as<R>(plugin: &Rc<str>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<str>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PLUGIN.set(self.0.take());
        }
    }
    let _restore = Restore(PLUGIN.replace(Some(plugin.clone())));
    f()
}

/// The plugin whose render runs now.
pub fn rendering() -> Option<Rc<str>> {
    PLUGIN.with_borrow(Clone::clone)
}

/// Runs `f` with `app` reachable from the bindings via [`with_app`]. Nests: an inner call
/// restores the outer app on return or unwind.
pub fn enter<R>(app: &App, f: impl FnOnce() -> R) -> R {
    struct Restore(*const App);
    impl Drop for Restore {
        fn drop(&mut self) {
            APP.set(self.0);
        }
    }
    let _restore = Restore(APP.replace(std::ptr::from_ref(app)));
    f()
}

/// Runs `f` with the active entry's `&App`, or returns `None` outside [`enter`].
///
/// The higher-ranked closure bound prevents the `&App` from escaping this call, so the shared
/// borrow is guaranteed to end before the enclosing [`enter`] restores the pointer and releases
/// its borrow.
pub fn with_app<R>(f: impl FnOnce(&App) -> R) -> Option<R> {
    let p = APP.get();
    if p.is_null() {
        return None;
    }
    // SAFETY: `p` is only ever written by `enter` from a live `&App` on this thread and restored
    // by a stack-local `Drop` guard before `enter` returns (including on unwind). Because `enter`
    // holds that shared `&App` borrow for its entire dynamic extent, no `&mut App` to the same
    // `App` can coexist in safe code; and because `f` is higher-ranked (`for<'a> FnOnce(&'a App)
    // -> R`), the reference cannot escape `with_app` and ends before `enter` returns.
    let app = unsafe { &*p };
    Some(f(app))
}

/// Something a plugin asked the app to do, carried out after the entry returns.
pub enum Request {
    OpenPanel { panel: String, instance: Option<String>, instant: bool },
    TogglePanel { panel: String, session: Option<String>, instant: bool },
    ClosePanel { panel: String },
    OpenPopover { id: String, anchor: String, trigger: Option<String>, side: rune::Side, align: rune::Align, gap: f32, data: rune_luau::Data, toggle: bool },
    ClosePopover,
    /// A step of the open popover's row list (`ui.flyoutRow`).
    Flyout(crate::plugin_host::FlyoutEvent),
    OpenDialog { id: String, data: rune_luau::Data },
    /// A nested view in the narrow More drawer (`Layout:OpenDrawerView`).
    DrawerView { title: String, data: rune_luau::Data },
    CloseDialog,
    Toast { variant: String, title: String, description: String, reveal: Option<(String, String)>, persistent: bool, buttons: Vec<crate::status_toast::PluginButton>, handle: u64 },
    /// Closes the toast Toast:Show answered with this handle, when it still shows.
    DismissToast(u64),
    /// Replaces what that toast shows, in place, while it shows (`ToastHandle:Update`).
    UpdateToast { handle: u64, variant: String, title: String, description: String, reveal: Option<(String, String)>, persistent: bool, buttons: Vec<crate::status_toast::PluginButton> },
    /// Opens a path in one of the apps `Workspace:OpenInApps` lists.
    OpenIn { app: String, path: String },
    OpenSettings(Option<String>),
    NewDraft { directory: String, prompt: String },
    /// A draft on a listed server (`Layout:OpenDraft`); `server` None for the window's.
    OpenDraft { server: Option<String>, directory: String, prompt: String },
    /// A folder on a server to open a draft in (`Layout:OpenProject`).
    OpenProject { server: String, title: String },
    RunCommand { id: String, input: Option<String> },
    OpenLink { href: String },
    OpenExternal(String),
    Reveal(String),
    CopyText(String),
    /// `session`: the notification's session, which a mute silences.
    Notify { title: String, body: String, session: Option<String> },
    Menu { anchor: String, menu: crate::popup::Menu, done: rune_luau::Completer },
    /// A developer tool (`Diagnostics:Set`).
    Dev { name: String, value: rune_luau::Data },
    /// A server source's list changed: the app checks the health of servers that came up.
    ServersChanged,
    Redraw,
}

/// Queues `request` from plugin `plugin` onto the active window's host.
pub fn ask(plugin: &Rc<str>, request: Request) {
    with_app(|app| app.plugins.pending_requests.borrow_mut().push((plugin.clone(), request)));
}

/// Queues `request` from plugin `plugin` directly onto `app`'s host (outside [`enter`]).
pub fn ask_in(app: &mut App, plugin: &Rc<str>, request: Request) {
    app.plugins.pending_requests.get_mut().push((plugin.clone(), request));
}

/// Takes what plugins asked `app` for since the last call.
pub fn take_asked(app: &mut App) -> Vec<(Rc<str>, Request)> {
    std::mem::take(app.plugins.pending_requests.get_mut())
}

/// Something a render read, whose change means the render must run again.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Dep {
    Session(String),
    Routed,
    Sessions,
    Model(String, String),
    Resource(Rc<str>, String),
    Store(Rc<str>, Rc<str>),
    Editor(Rc<str>, usize),
    Popover(Rc<str>, String),
    /// Which row's list the plugin's open popover shows beside it.
    Flyout(Rc<str>),
    Dialog(Rc<str>, String),
    Panel(Rc<str>, String),
    Narrow,
    Locale,
    Server,
    /// The listed servers and their health.
    Servers,
    Contract(String),
    /// A developer setting (`Diagnostics:Get`).
    Dev(String),
    /// The console's lines.
    Console,
    /// A session's changes (`Diff`).
    Diff(String),
    /// Whether `Diff` draws split or unified (`Diff:Mode`).
    DiffMode,
    /// A session's changed file count and line totals (`DiffDocument:Totals`).
    DiffTotals(String),
    /// A session's change tree and file change kinds (`FileTreeModel:Version` on a diff).
    DiffTree(String),
    /// A session's change status, source, active path and available sources.
    DiffMeta(String),
    /// A session's narrow Changes list: the files open in it and the line comments drawn in it.
    NarrowReview(String),
    /// A session's workspace files: its tree, finds and open files (`Workspace`).
    Files(String),
    /// Whether one open file is missing on disk (`CodeDocument:Missing`).
    FilesMissing(String, String),
    /// A session's workspace tree and file search (`FileTreeModel`).
    FilesTree(String),
    /// One open document's content, find bar, media and scroll (`CodeDocument`, `ui.codeView`).
    FilesDoc(String, String),
    /// A session's terminals (`Terminal`).
    Terminals(String),
    /// What a session's terminal surfaces and labels draw.
    TerminalScreen(String),
    /// The shared panel sidebar.
    Sidebar,
    /// A plugin's own state behind a render nested in another's (a file viewer inside the Files`n    /// panel): its script's signals and its contributions.
    Plugin(Rc<str>),
    /// An Office file's views (Office:Version).
    Office(u64),
    /// Where the app's updates stand (`Updater:State`).
    Updater,
    /// The window's browser: its pages, the sessions they belong to, where the keyboard is
    /// among them, and the engine's state (`Browser`).
    Browser,
    /// Where a plugin's image stands (`Images:State`, `ui.image`).
    Image(Rc<str>, u32),
    /// Whether a plugin's video (`ui.video`) can play, by its URL.
    Video(Rc<str>, String),
}

/// Records that the running render read `dep`. Outside a recorded render it does nothing.
pub fn read(dep: Dep) {
    READS.with_borrow_mut(|r| {
        if let Some(r) = r
            && !r.contains(&dep)
        {
            r.push(dep);
        }
    });
}

/// Runs `f`, returning what it read alongside its result. Nested recordings each get their own.
pub fn recording<R>(f: impl FnOnce() -> R) -> (R, Vec<Dep>) {
    struct Restore(Option<Vec<Dep>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            READS.set(self.0.take());
        }
    }
    let mut restore = Restore(READS.replace(Some(Vec::new())));
    let out = f();
    let reads = READS.replace(restore.0.take()).unwrap_or_default();
    std::mem::forget(restore);
    (out, reads)
}

/// A reactive function's last answer: the script's version it was asked at, what the call read
/// and how that read then.
pub(crate) struct Answer {
    asked: (Option<u64>, u64, u64),
    reads: Vec<Dep>,
    read: u64,
    value: rune_luau::Data,
}

/// A plugin function's answer that the host reads as it lists things (a command's `enabled`, a
/// panel's `enabled`), from `ask`, which calls into `script`. It is answered from the last call
/// under `key` while the script hasn't run since and nothing that call read changed, so a list
/// built every frame enters no VM.
pub fn answer(app: &App, asked: (Option<u64>, u64, u64), key: String, ask: impl FnOnce() -> rune_luau::Data) -> rune_luau::Data {
    let hit = app.plugins.command_answers.borrow().get(&key).filter(|a| a.asked == asked && stamp(app, &a.reads) == a.read).map(|a| a.value.clone());
    if let Some(value) = hit {
        return value;
    }
    let (value, reads) = recording(|| enter(app, ask));
    let read = stamp(app, &reads);
    app.plugins.command_answers.borrow_mut().insert(key, Answer { asked, reads, read, value: value.clone() });
    value
}

/// A hash of what `deps` read now: equal while none of them changed.
pub fn stamp(app: &App, deps: &[Dep]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    for dep in deps {
        crate::plugin_host::services::dep_version(app, dep).hash(&mut h);
    }
    h.finish()
}
