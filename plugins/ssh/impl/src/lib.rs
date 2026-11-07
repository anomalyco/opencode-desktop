//! Desktop's built-in SSH plugin (`packages/gui-plugins/src/ssh`) as a native plugin: OpenCode
//! servers on hosts you reach over SSH. A host's service is found (or installed and started),
//! a local port is forwarded to it, and once it answers it joins the app's servers.
//!
//! - The app half (Desktop's `main.ts`) owns the connections: the machine, each attempt's `ssh`
//!   processes and tunnel, and the sign-in listener `ssh`'s askpass helper (the app's own
//!   executable) reaches. It provides them to the window as the `ssh.servers` service.
//! - The window half (`renderer.tsx`) adds the server source (each host's row in Settings ›
//!   Servers), Add SSH server (`server.add` and the palette), Connect and Authenticate
//!   (`server.row`), and the dialog its prompts show in.
//!
//! A built-in: this rlib declares it, `plugins/ssh` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod askpass;
mod command;
mod controller;
mod dialog;
mod machine;
mod row;

use opencode_plugin::prelude::*;

use controller::{Core, CoreRef};
use dialog::{DialogState, Fields, Mode};
use machine::{Config, Stage};

/// The server source's id: servers are `ssh:<id>` in the app.
pub const SOURCE: &str = "ssh";
const DIALOG: &str = "connect";

/// The connections, shared with the window half.
pub struct Handle(CoreRef);

// SAFETY: the host calls both halves, and drops the service, on the app's thread only.
#[allow(unsafe_code)]
unsafe impl Send for Handle {}
// SAFETY: as above.
#[allow(unsafe_code)]
unsafe impl Sync for Handle {}

const SERVICE: Service<Handle> = Service::new("ssh.servers", 1);

/// `sshName`: the name given, or the host the target ends with.
pub fn name(config: &Config) -> String {
    if config.name.trim().is_empty() { command::hostname(&config.target) } else { config.name.trim().to_owned() }
}

/// A random UUID-shaped id (`uuid()`), for a new server and the listener's token.
pub fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    for (i, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

pub struct SshApp {
    core: CoreRef,
    started: bool,
}

impl AppPlugin for SshApp {
    fn setup(cx: &mut AppSetupCx<Self>) -> Result<Self, PluginError> {
        let core = Core::new();
        if !cx.provide_service(&SERVICE, Handle(core.clone())) {
            return Err(PluginError::new("couldn't provide ssh.servers"));
        }
        let waker = cx.waker();
        waker.wake();
        core.borrow_mut().waker = Some(waker);
        Ok(SshApp { core, started: false })
    }

    /// The saved hosts restore once; then the listeners' prompts land and due retries run.
    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let mut core = self.core.borrow_mut();
        if !std::mem::replace(&mut self.started, true) {
            core.publish(cx);
            core.restore(cx);
        }
        let before = core.revision;
        core.notes(cx);
        if let Some(next) = core.due(cx) {
            cx.wake_after(next.max(1));
        }
        core.revision != before
    }
}

pub struct SshWindow {
    live: Live<Handle>,
    pub(crate) fields: Fields,
    pub(crate) dialog: Option<DialogState>,
    /// A connect this window started from a row, whose prompts open the dialog.
    pub(crate) prompting: Option<String>,
    seen: u64,
}

impl SshWindow {
    pub(crate) fn core(&self) -> Option<CoreRef> {
        self.live.with(|h| h.0.clone())
    }

    /// Opens the prompt dialog for the row's connect once it asks something (`dialog.next`).
    pub(crate) fn prompt_dialog(&mut self, cx: &mut ActionCx) {
        let (Some(id), Some(core)) = (self.prompting.clone(), self.core()) else { return };
        if cx.dialog_open(DIALOG) {
            return;
        }
        let record = core.borrow().state.servers.iter().find(|r| r.config.id == id).cloned();
        match record {
            Some(r) if r.prompt().is_some() || r.stage() == Stage::Incompatible => {
                self.prompting = None;
                self.open_dialog(cx, Mode::Prompt, Some(r.config), false);
            }
            Some(r) if r.stage() == Stage::Connecting => {}
            _ => self.prompting = None,
        }
    }
}

impl WindowPlugin for SshWindow {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        let live = cx.consume(&SERVICE);
        let fields = Fields { target: cx.create_editor(""), name: cx.create_editor(""), response: cx.create_editor("") };
        cx.add_server_source(Contribution::server_source(SOURCE, "SSH").order(2), row::server_row);
        cx.add_menu_item(Contribution::menu_item(Menu::ServerAdd, "add", "Add SSH server").order(1), |s, cx| s.open_dialog(cx, Mode::Add, None, false));
        cx.add_command(CommandSpec::new("add", "Add SSH server").keywords("server ssh remote host"), |s, cx| s.open_dialog(cx, Mode::Add, None, true));
        cx.add_menu_item(Contribution::menu_item(Menu::ServerRow, "connect", "Connect"), |s, cx| {
            let key = cx.input().to_owned();
            if let Some(id) = key.strip_prefix("ssh:") {
                s.connect(cx, id);
            }
        });
        cx.add_dialog(Contribution::dialog(DIALOG, "Add SSH server"), dialog::render, |s, cx| s.dismissed(cx));
        Ok(SshWindow { live, fields, dialog: None, prompting: None, seen: 0 })
    }

    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let Some(core) = self.core() else { return false };
        let revision = core.borrow().revision;
        if revision == self.seen {
            return false;
        }
        self.seen = revision;
        if cx.dialog_open(DIALOG) {
            self.sync(cx);
        } else {
            self.prompt_dialog(cx);
        }
        true
    }
}

opencode_plugin::declare_plugin!(app = SshApp, window = SshWindow);
