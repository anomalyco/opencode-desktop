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
    pub render: RenderEnv,
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
    pub run: Option<CallbackEnv>,
    pub render: Option<RenderEnv>,
    pub query: Option<QueryEnv>,
}

/// One server a server source listed.
#[derive(Clone, Debug)]
pub struct ServerRec {
    pub source: String,
    pub id: String,
    pub name: String,
    pub state: u32,
    pub url: String,
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
    /// The app half's state; null without one.
    pub app: *mut c_void,
    /// The window half's state; null without one.
    pub state: *mut c_void,
    pub contributions: Contributions,
    /// What its server sources list now.
    pub servers: Vec<ServerRec>,
    /// When `poll` is due next (`wake_after`), on the app clock.
    pub deadline: Option<f64>,
    /// Dropped last, after the states and every callback.
    pub lib: Arc<LoadedLibrary>,
}

impl Instance {
    pub fn state_of(&self, scope: Scope) -> *mut c_void {
        match scope {
            Scope::App => self.app,
            Scope::Window => self.state,
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Contributions first: their captures may point into the states.
        self.contributions = Contributions::default();
        let halves = [(self.vtable.drop_state, std::mem::replace(&mut self.state, std::ptr::null_mut())), (self.vtable.app_drop_state, std::mem::replace(&mut self.app, std::ptr::null_mut()))];
        for (drop_state, state) in halves {
            let (Some(drop_state), false) = (drop_state, state.is_null()) else { continue };
            // SAFETY: the state this instance's setup returned, dropped once; the plugin catches a panic.
            let out = unsafe { drop_state(state) };
            if let Err((_, message)) = out.into_result() {
                super::tasks::log(format!("opencode-plugin: dropping a plugin's state failed: {message}"));
            }
        }
    }
}