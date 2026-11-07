//! The app half of a plugin: one instance per app process (Electron's `main` entry, without the
//! IPC). It owns what outlives a window (workers, sockets, tunnels) and publishes services the
//! window halves of any plugin use in-process.

use core::marker::PhantomData;

use crate::cx::{PluginError, PollCx};

/// A plugin's process-wide state, built once before any window half sets up.
pub trait AppPlugin: Sized + 'static {
    fn setup(cx: &mut AppSetupCx<Self>) -> Result<Self, PluginError>;

    /// As `WindowPlugin::poll`: runs only when this plugin's waker fired.
    fn poll(&mut self, _cx: &mut PollCx) -> bool {
        false
    }
}

/// Publishes services and app menu items. The methods every setup shares are in
/// [`crate::shared`]; `add_menubar_item` in [`crate::contrib`].
pub struct AppSetupCx<A> {
    pub(crate) _a: PhantomData<fn(&mut A)>,
}
