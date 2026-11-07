//! Desktop's built-in WSL plugin (`packages/gui-plugins/src/wsl`) as a native plugin, for
//! Windows: an OpenCode server in each WSL distro you add, started with the app and watched,
//! listed among the app's servers once it answers, so the window can work in the distro's
//! projects with its Linux paths.
//!
//! - The app half (Desktop's `main.ts`) owns the controller: the machine, `wsl.exe` probes and
//!   installs, and each distro's server, all as processes the host supervises. It provides it to
//!   the window as the `wsl.servers` service.
//! - The window half (`renderer.tsx`) adds the server source (each server's row in Settings ›
//!   Servers), Add WSL server (`server.add`), Retry start (`server.row`) and the add dialog.
//!
//! A built-in: this rlib declares it, `plugins/wsl` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod contract;
mod controller;
mod dialog;
mod machine;
mod model;
mod row;
mod runtime;
mod sidecar;
mod view;

use std::cell::RefCell;
use std::rc::Rc;

use opencode_plugin::prelude::*;

use controller::{Core, CoreRef};
use dialog::{DialogState, Op, Ui, UiRef};

/// The server source's id: servers are `wsl:<distro>` in the app.
pub const SOURCE: &str = "wsl";
const DIALOG: &str = "add";

/// The controller, shared with the window half.
pub struct Handle(CoreRef);

// SAFETY: the host calls both halves, and drops the service, on the app's thread only.
#[allow(unsafe_code)]
unsafe impl Send for Handle {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Handle {}

const SERVICE: Service<Handle> = Service::new("wsl.servers", 1);

pub struct WslApp {
    core: CoreRef,
    started: bool,
}

impl AppPlugin for WslApp {
    fn setup(cx: &mut AppSetupCx<Self>) -> Result<Self, PluginError> {
        let core = Core::new();
        if !cx.provide_service(&SERVICE, Handle(core.clone())) {
            return Err(PluginError::new("couldn't provide wsl.servers"));
        }
        let entries = core.borrow().entries();
        cx.set_servers(SOURCE, &entries, true);
        // The saved distros' servers start on the first poll, which has a handler's context.
        let waker = cx.waker();
        waker.wake();
        core.borrow_mut().waker = Some(waker);
        Ok(WslApp { core, started: false })
    }

    fn poll(&mut self, cx: &mut PollCx) -> bool {
        if std::mem::replace(&mut self.started, true) {
            return false;
        }
        controller::with(&self.core, cx, |c, cx| {
            c.start_configured(cx);
            c.publish(cx);
        });
        true
    }
}

pub struct WslWindow {
    live: Live<Handle>,
    ui: UiRef,
    /// The controller's revision this window last planned probes for.
    seen: u64,
}

impl WslWindow {
    pub(crate) fn core(&self) -> Option<CoreRef> {
        self.live.with(|h| h.0.clone())
    }

    /// A user action in the dialog (`dialog-machine.ts` events).
    pub(crate) fn act(&mut self, cx: &mut ActionCx, f: impl FnOnce(&mut DialogState, &model::Model) -> Option<Op>) {
        let Some(core) = self.core() else { return cx.toast_error("Request failed", "WSL is unavailable") };
        let search = cx.editor_text(self.ui.borrow().search);
        dialog::act(&self.ui, &core, cx, &search, f);
    }

    fn check_again(&mut self, cx: &mut ActionCx) {
        dialog::clear_failures(&self.ui);
        self.act(cx, |d, m| (d.status == dialog::Status::Main && !m.busy).then_some(Op::RefreshDistros));
    }

    fn open(&mut self, cx: &mut ActionCx) {
        dialog::reset(&self.ui);
        let search = self.ui.borrow().search;
        cx.set_editor_text(search, "");
        cx.open_dialog(DIALOG);
        self.act(cx, |_, _| None);
    }
}

impl WindowPlugin for WslWindow {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        view::intern_icons();
        let live = cx.consume(&SERVICE);
        let search = cx.create_editor("");
        cx.add_server_source(Contribution::server_source(SOURCE, "WSL").order(1), row::server_row);
        cx.add_menu_item(Contribution::menu_item(Menu::ServerAdd, "add", "Add WSL server").order(2), |s, cx| s.open(cx));
        cx.add_menu_item(Contribution::menu_item(Menu::ServerRow, "retry", "Retry start"), |s, cx| {
            let key = cx.input().to_owned();
            if let Some(distro) = key.strip_prefix("wsl:") {
                s.retry(cx, distro);
            }
        });
        cx.add_dialog(Contribution::dialog(DIALOG, "Add WSL server"), view::dialog, |_, _| {});
        Ok(WslWindow { live, ui: Rc::new(RefCell::new(Ui { dialog: DialogState::default(), probes: Default::default(), search })), seen: 0 })
    }

    /// The controller changed: the dialog plans its next probe, and the window redraws.
    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let Some(core) = self.core() else { return false };
        let revision = core.borrow().revision;
        if revision == self.seen {
            return false;
        }
        self.seen = revision;
        if cx.dialog_open(DIALOG) {
            let search = cx.editor_text(self.ui.borrow().search);
            dialog::sync(&self.ui, &core, cx, &search);
        }
        true
    }
}

opencode_plugin::declare_plugin!(app = WslApp, window = WslWindow);
