//! A loaded plugin's instance: its app and window states, what they contributed, and the
//! callbacks that keep its library mapped.

use std::ffi::c_void;
use std::sync::Arc;

use opencode_plugin::EditorHandle;
use opencode_plugin::abi::{Callback, PluginVtable, QueryCallback, RenderCallback};

use super::loader::LoadedLibrary;

/// A plugin callback the host owns: its captures are freed (by the plugin's own destructor)
/// when the last holder drops it, while the library is still mapped.
pub struct CallbackEnv {
    pub cb: Callback,
    /// Keeps the library mapped while the callback lives.
    _lib: Arc<LoadedLibrary>,
}

impl CallbackEnv {
    pub fn new(cb: Callback, lib: Arc<LoadedLibrary>) -> Self {
        CallbackEnv { cb, _lib: lib }
    }
}

impl Drop for CallbackEnv {
    fn drop(&mut self) {
        if let Some(drop_data) = self.cb.drop_data.take() {
            // SAFETY: the plugin's destructor for this callback's captures, called once.
            unsafe { drop_data(self.cb.data) };
        }
    }
}

pub struct RenderEnv {
    pub cb: RenderCallback,
    /// Keeps the library mapped while the callback lives.
    _lib: Arc<LoadedLibrary>,
}

impl RenderEnv {
    pub fn new(cb: RenderCallback, lib: Arc<LoadedLibrary>) -> Self {
        RenderEnv { cb, _lib: lib }
    }
}

impl Drop for RenderEnv {
    fn drop(&mut self) {
        if let Some(drop_data) = self.cb.drop_data.take() {
            // SAFETY: as `CallbackEnv`.
            unsafe { drop_data(self.cb.data) };
        }
    }
}

pub struct QueryEnv {
    pub cb: QueryCallback,
    /// Keeps the library mapped while the callback lives.
    _lib: Arc<LoadedLibrary>,
}

impl QueryEnv {
    pub fn new(cb: QueryCallback, lib: Arc<LoadedLibrary>) -> Self {
        QueryEnv { cb, _lib: lib }
    }
}

impl Drop for QueryEnv {
    fn drop(&mut self) {
        if let Some(drop_data) = self.cb.drop_data.take() {
            // SAFETY: as `CallbackEnv`.
            unsafe { drop_data(self.cb.data) };
        }
    }
}

/// Which half of a plugin a callback runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    App,
    Window,
}

/// A press handler inside a drawn element. It runs only while its instance generation is the
/// live one, so an element a reload left behind does nothing.
pub struct Press {
    pub slot: usize,
    pub generation: u32,
    pub env: CallbackEnv,
}

pub struct CommandRec {
    /// `<plugin>.<id>`, interned: the palette lists commands by `&'static str`.
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub slash: Option<&'static str>,
    pub keybind: Option<rune::Keybind>,
    pub editable: bool,
    pub hidden: bool,
    /// Its Settings › Shortcuts group (`CommandSpecAbi::section`).
    pub section: Option<crate::keymap::Section>,
    /// /name words runs it with words (slash.arguments).
    pub slash_args: bool,
    /// Offered only while a session is open in a desktop-width window.
    pub needs_session: bool,
    pub run: CallbackEnv,
}

pub struct PanelRec {
    pub id: String,
    /// `<plugin>.<id>`, interned: the tab id under the host's `plugin` panel, and its launcher id.
    pub tab: &'static str,
    pub title: String,
    pub description: String,
    pub icon: String,
    pub order: i32,
    /// `PANEL_*` flags.
    pub flags: u32,
    pub render: RenderEnv,
    /// Draws the tab's label, in place of its icon and title.
    pub label: Option<RenderEnv>,
    /// PANEL_INSTANCES: which open instances it lists, with their titles.
    pub list: Option<QueryEnv>,
    /// PANEL_INSTANCES: runs as an instance's tab closes.
    pub close: Option<CallbackEnv>,
}

pub struct SettingRec {
    pub id: String,
    pub title: String,
    pub description: String,
    pub keywords: String,
    pub render: RenderEnv,
}

/// Any contribution of `opencode_plugin::abi::contrib`'s kinds.
pub struct ContribRec {
    pub kind: u32,
    pub scope: Scope,
    pub id: String,
    /// `<plugin>.<id>`, interned: how menus and actions name it.
    pub key: &'static str,
    pub title: String,
    pub description: String,
    pub keywords: String,
    pub icon: String,
    pub target: String,
    pub order: i32,
    pub flags: u32,
    pub run: Option<CallbackEnv>,
    pub render: Option<RenderEnv>,
    pub query: Option<QueryEnv>,
}

/// One server a server source listed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerRec {
    pub source: String,
    pub id: String,
    pub name: String,
    /// Its kind's badge (`WSL`, `SSH`).
    pub label: String,
    /// `SERVER_*`.
    pub state: u32,
    pub url: String,
    pub password: String,
    /// It joins the app's servers rather than only showing in Settings.
    pub listed: bool,
    /// Its plugin keeps it reachable (an SSH tunnel): a check of its new address shows the
    /// connecting spinner (`ServerEntry.managed`).
    pub managed: bool,
}

/// What a setup registered.
#[derive(Default)]
pub struct Contributions {
    pub commands: Vec<CommandRec>,
    pub panels: Vec<PanelRec>,
    pub settings: Vec<SettingRec>,
    pub contribs: Vec<ContribRec>,
    pub editors: Vec<EditorHandle>,
}

impl Contributions {
    pub fn of(&self, kind: u32) -> impl Iterator<Item = &ContribRec> {
        self.contribs.iter().filter(move |c| c.kind == kind)
    }
}

pub struct Instance {
    pub generation: u32,
    pub vtable: PluginVtable,
    /// The app half, shared by every window's instance of this library; none without one.
    pub app: Option<std::rc::Rc<super::app_half::AppHalf>>,
    /// The window half's state; null without one.
    pub state: *mut c_void,
    pub contributions: Contributions,
    /// What its server sources list now.
    pub servers: Vec<ServerRec>,
    /// Its server sources that have read what they list (`Server.ready`).
    pub ready_sources: Vec<String>,
    /// When `poll` is due next (`wake_after`), on the app clock.
    pub deadline: Option<f64>,
    /// Dropped last, after the states and every callback.
    pub lib: Arc<LoadedLibrary>,
}

impl Instance {
    /// Replaces what source `source` lists; true when that changed anything.
    pub fn set_servers(&mut self, source: &str, list: Vec<ServerRec>, ready: bool) -> bool {
        let before: Vec<&ServerRec> = self.servers.iter().filter(|s| s.source == source).collect();
        let was_ready = self.ready_sources.iter().any(|s| s == source);
        if before.len() == list.len() && before.iter().zip(&list).all(|(a, b)| *a == b) && was_ready == ready {
            return false;
        }
        self.servers.retain(|s| s.source != source);
        self.servers.extend(list);
        self.ready_sources.retain(|s| s != source);
        if ready {
            self.ready_sources.push(source.to_owned());
        }
        true
    }

    /// Its contributions of `kind`, the window half's and the app half's.
    pub fn of(&self, kind: u32) -> impl Iterator<Item = &ContribRec> {
        let app = self.app.iter().flat_map(move |half| half.contribs.iter().filter(move |c| c.kind == kind));
        self.contributions.of(kind).chain(app)
    }

    pub fn state_of(&self, scope: Scope) -> *mut c_void {
        match scope {
            Scope::App => self.app.as_ref().map_or(std::ptr::null_mut(), |half| half.state),
            Scope::Window => self.state,
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Contributions first: their captures may point into the states.
        self.contributions = Contributions::default();
        let state = std::mem::replace(&mut self.state, std::ptr::null_mut());
        if let (Some(drop_state), false) = (self.vtable.drop_state, state.is_null()) {
            // SAFETY: the state this instance's setup returned, dropped once; the plugin catches a panic.
            let out = unsafe { drop_state(state) };
            if let Err((_, message)) = out.into_result() {
                super::tasks::log(format!("opencode-plugin: dropping a plugin's state failed: {message}"));
            }
        }
        // The app half goes with the last window's instance.
        self.app = None;
    }
}