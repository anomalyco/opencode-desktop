//! A loaded plugin's window instance: its state, its contributions, and the callbacks that
//! keep its library mapped.

use std::ffi::c_void;
use std::sync::Arc;

use opencode_plugin::EditorHandle;
use opencode_plugin::abi::{Callback, PluginVtable, RenderCallback};

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
    pub keybind: Option<rune::Keybind>,
    pub editable: bool,
    pub hidden: bool,
    pub run: CallbackEnv,
}

pub struct PanelRec {
    pub id: String,
    /// `<plugin>.<id>`, the tab id under the host's `plugin` panel.
    pub tab: String,
    pub title: String,
    pub icon: String,
    pub order: i32,
    pub render: RenderEnv,
}

pub struct SettingRec {
    pub id: String,
    pub title: String,
    pub description: String,
    pub render: RenderEnv,
}

/// What a setup registered.
#[derive(Default)]
pub struct Contributions {
    pub commands: Vec<CommandRec>,
    pub panels: Vec<PanelRec>,
    pub settings: Vec<SettingRec>,
    pub editors: Vec<EditorHandle>,
}

pub struct Instance {
    pub generation: u32,
    pub vtable: PluginVtable,
    pub state: *mut c_void,
    pub contributions: Contributions,
    /// When `poll` is due next (`wake_after`), on the app clock.
    pub deadline: Option<f64>,
    /// Dropped last, after the state and every callback.
    pub lib: Arc<LoadedLibrary>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.state.is_null() {
            return;
        }
        // SAFETY: the state this instance's setup returned, dropped once; the plugin catches a panic.
        let out = unsafe { (self.vtable.drop_state)(self.state) };
        self.state = std::ptr::null_mut();
        if let Err((_, message)) = out.into_result() {
            eprintln!("opencode-plugin: dropping a plugin's state failed: {message}");
        }
    }
}
