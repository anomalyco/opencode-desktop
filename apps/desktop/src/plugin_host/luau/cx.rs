//! How a binding reaches the app. Every entry into a plugin's VM happens inside [`with_app`]:
//! renders and queries read the window's `App` through [`app`]; what a binding wants changed
//! (open a panel, show a toast) it queues with [`ask`], and the host carries the queue out with
//! `&mut App` and the transaction as soon as the entry returns ([`super::requests::carry_out`]).
//! A render also records what it read ([`read`]), so the host knows when to run it again.

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::App;

thread_local! {
    static APP: Cell<*const App> = const { Cell::new(std::ptr::null()) };
    static ASKED: RefCell<Vec<(Rc<str>, Request)>> = const { RefCell::new(Vec::new()) };
    static READS: RefCell<Option<Vec<Dep>>> = const { RefCell::new(None) };
    static PLUGIN: RefCell<Option<Rc<str>>> = const { RefCell::new(None) };
}

/// Runs `f` as plugin `plugin`'s render: nodes that act for their plugin (`ui.flyoutRow`)
/// read it with [`rendering`].
pub fn rendering_as<R>(plugin: &Rc<str>, f: impl FnOnce() -> R) -> R {
    let outer = PLUGIN.replace(Some(plugin.clone()));
    let out = f();
    PLUGIN.set(outer);
    out
}

/// The plugin whose render runs now.
pub fn rendering() -> Option<Rc<str>> {
    PLUGIN.with_borrow(Clone::clone)
}

/// Runs `f` with `app` reachable from the bindings. Nests: an inner call restores the outer app.
pub fn with_app<R>(app: &App, f: impl FnOnce() -> R) -> R {
    struct Restore(*const App);
    impl Drop for Restore {
        fn drop(&mut self) {
            APP.set(self.0);
        }
    }
    let _restore = Restore(APP.replace(app));
    f()
}

/// The app of the entry running now; `None` outside one.
pub fn app<'a>() -> Option<&'a App> {
    let p = APP.get();
    // SAFETY: `with_app` sets the pointer from a live reference for exactly the span of its call
    // and restores the previous one after it, and the bindings that read it run inside that span
    // on this thread; nothing mutates the app while it is set.
    (!p.is_null()).then(|| unsafe { &*p })
}

/// Something a plugin asked the app to do, carried out after the entry returns.
pub enum Request {
    OpenPanel { panel: String, instance: Option<String>, instant: bool },
    TogglePanel { panel: String, session: Option<String>, instant: bool },
    ClosePanel { panel: String },
    OpenPopover { id: String, anchor: String, side: rune::Side, align: rune::Align, gap: f32, data: rune_luau::Data, toggle: bool },
    ClosePopover,
    /// A step of the open popover's row list (`ui.flyoutRow`).
    Flyout(super::FlyoutEvent),
    OpenDialog { id: String, data: rune_luau::Data },
    CloseDialog,
    Toast { variant: String, title: String, description: String, reveal: Option<(String, String)>, persistent: bool },
    OpenSettings(Option<String>),
    NewDraft { directory: String, prompt: String },
    RunCommand { id: String, input: Option<String> },
    OpenLink { href: String },
    OpenExternal(String),
    Reveal(String),
    CopyText(String),
    Notify { title: String, body: String },
    Menu { anchor: String, menu: crate::popup::Menu, done: rune_luau::Completer },
    /// A developer tool (`Diagnostics:Set`).
    Dev { name: String, value: rune_luau::Data },
    /// A server source's list changed: the app checks the health of servers that came up.
    ServersChanged,
    Redraw,
}

/// Queues `request` from plugin `plugin`.
pub fn ask(plugin: &Rc<str>, request: Request) {
    ASKED.with_borrow_mut(|q| q.push((plugin.clone(), request)));
}

/// Takes what plugins asked for since the last call.
pub fn take_asked() -> Vec<(Rc<str>, Request)> {
    ASKED.with_borrow_mut(std::mem::take)
}

/// Something a render read, whose change means the render must run again.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Dep {
    Session(String),
    Routed,
    Sessions,
    Model(String, String),
    Resource(Rc<str>, String),
    Store(Rc<str>, String),
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
    /// A session's workspace files: its tree, finds and open files (`Workspace`).
    Files(String),
    /// The shared panel sidebar.
    Sidebar,
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
    let outer = READS.replace(Some(Vec::new()));
    let out = f();
    let reads = READS.replace(outer).unwrap_or_default();
    (out, reads)
}

/// A hash of what `deps` read now: equal while none of them changed.
pub fn stamp(app: &App, deps: &[Dep]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    for dep in deps {
        super::services::dep_version(app, dep).hash(&mut h);
    }
    h.finish()
}
