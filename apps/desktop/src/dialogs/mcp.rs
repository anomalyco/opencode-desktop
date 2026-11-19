//! Desktop host adapter for `opencode-feature-mcp` (`features/mcp`).

pub use opencode_feature_mcp::*;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, OverlayRegistration};
use opencode_kit::integration::McpServer;
use rune::machine::Attempt;
use rune::{KeyEvent, Tx};

use crate::App;
use crate::jobs::Apply;
use crate::keymap::Bind;
use crate::pickers::PickerHost as _;

impl McpHost for App {
    fn mcp(&self) -> &Mcp {
        &self.dialogs.mcp
    }

    fn mcp_mut(&mut self) -> &mut Mcp {
        &mut self.dialogs.mcp
    }

    fn active_directory(&self) -> String {
        match &*self.route {
            crate::Route::Session { id, .. } => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir().to_owned(),
        }
    }

    fn can_write(&self) -> bool {
        App::can_write(self)
    }

    fn is_toggle_bind(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        Bind::McpToggle.matches_tx(ev, tx)
    }

    fn can_toggle_shortcut(&self) -> bool {
        !self.dialogs.connect.is_modal_open()
    }

    fn before_dialog_open(&mut self) {
        self.close_picker(false);
        self.composer.focused = false;
    }

    fn auto_focus_composer(&mut self, tx: &mut Tx) {
        if crate::auto_focus_composer(tx) {
            self.composer.focused = self.has_composer() && !self.settings.open && self.dialogs.mcp.open.is_none() && self.dialogs.mcp.sign_in.is_none();
        }
    }

    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        crate::open_in::open_external(tx, url);
    }

    fn spawn_job(&mut self, job: McpJob) {
        match job {
            McpJob::Server { delay, request } => {
                self.job_after(delay, move |srv| {
                    let reply = execute_request(srv, request);
                    Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
                });
            }
            McpJob::Timer { delay, reply } => {
                self.job_after(delay, move |_| {
                    Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
                });
            }
            McpJob::Defer { reply } => {
                self.later(Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply);
            }
        }
    }

    fn on_toggle_settled(&mut self, dir: &str, attempt: Attempt, ok: bool, tx: &mut Tx) {
        self.start_mcp_settled(dir, attempt, ok, tx);
    }
}

impl App {
    /// `dir`'s servers by name, once read.
    pub(crate) fn mcp_servers(&self, dir: &str) -> Option<&[McpServer]> {
        servers(self, dir)
    }

    /// Reads `dir`'s servers again.
    pub(crate) fn sync_mcp(&mut self, dir: &str) {
        sync(self, dir);
    }

    /// The toggles in flight in `dir`, which a new session there waits for (`toggling`).
    pub(crate) fn mcp_toggling_in(&self, dir: &str) -> Vec<Attempt> {
        toggling_in(self, dir)
    }

    /// Opens the MCPs dialog on the open session's or draft's folder, or closes it.
    pub fn toggle_mcp_dialog(&mut self, tx: &mut Tx) {
        toggle_dialog(self, tx);
    }

    /// Opens the dialog on `dir`; `server` opens it straight on that server's error.
    pub fn open_mcp_dialog(&mut self, dir: String, server: Option<&str>, tx: &mut Tx) {
        open_dialog(self, dir, server, tx);
    }

    pub fn close_mcp_dialog(&mut self, tx: &mut Tx) {
        close_dialog(self, tx);
    }

    /// Connects (`enabled`) or disconnects a server, or starts its sign-in when it needs one
    /// (`mcp.tsx` `toggle`). One toggle per server at a time; a connecting server takes none.
    pub fn toggle_mcp(&mut self, dir: &str, name: &str, enabled: bool) {
        toggle_server(self, dir, name, enabled);
    }
}

/// Kernel feature registration for the MCPs and browser OAuth sign-in dialogs.
pub struct McpFeature;

impl Feature<App> for McpFeature {
    fn id(&self) -> &'static str {
        "feature.mcp"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_command(
            CommandEntry::<App>::from_key(
                "mcp.toggle",
                opencode_i18n::key!("command.mcp.toggle"),
                crate::commands::order::MCP_TOGGLE,
                |s, tx| s.toggle_mcp_dialog(tx),
            )
            .describe(opencode_i18n::key!("command.mcp.toggle.description"))
            .slash("mcp")
            .bind_id(Bind::McpToggle.id()),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(OVERLAY_ID)
                .with_z(420)
                .with_key_priority(110)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    view::layer(s, p, cx)
                })
                .with_key(|s, ev, tx| key(s, ev, tx))
                .with_dismiss(|s, tx| close_dialog(s, tx)),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(SIGN_IN_OVERLAY_ID)
                .with_z(430)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    sign_in::layer(s, p, cx)
                })
                .with_dismiss(|s, tx| sign_in::close(s, tx)),
        );
    }
}

// `--scene mcp-sign-in-long`: a sign-in that failed with an error longer than the dialog, which
// scrolls in it.
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("mcp-sign-in-long", |s| {
        let error = (1..=30)
            .map(|i| format!("at frame {i}: invalid_grant from https://auth.example.com/token"))
            .collect::<Vec<_>>()
            .join("\n");
        s.dialogs.mcp.sign_in = Some(sign_in::SignIn::failed_auth_preview(
            String::new(),
            "linear".into(),
            "linear".into(),
            "msi_scene".into(),
            "https://auth.example.com/authorize".into(),
            error,
        ));
    }),
]);
