//! Connect an integration (Desktop's `DialogConnectProvider`, `providers/connect/*`): the
//! searchable list of providers and services, then the picked one's sign-in (an API key, a
//! browser sign-in, a device code, a command or an external login, with the method's form), its
//! accounts once connected, and the models to start with. `+` beside the model search, `/connect`
//! and Settings › Providers open it. The steps are [`dialog`] and [`machine`]; [`run`] executes
//! their effects against the server through [`ConnectHost`].

#![forbid(unsafe_code)]

pub mod catalog;
pub mod dialog;
pub mod form;
pub mod machine;
pub mod picker_reads;
pub mod run;
pub mod steps;
mod ui;
pub mod view;
pub mod welcome;

use rune::machine::{Attempt, Effects};
use rune::{Key, KeyEvent, TextEdit, Tx};
use serde_json::Value;
use session_view::pickers::Model;

pub use catalog::{
    CatalogRead, Catalogs, ProviderInfo, SyncAfter, integration_of, integrations, provider_infos, seed_catalogs, sync_catalogs,
};
pub use picker_reads::{PickerList, PickerRead};
pub use run::{ConnectJob, ConnectReply, ConnectRequest, apply_reply, execute_request};

use dialog::{Accounts, Dialog, Event, Provider, Step};

/// Overlay stack identifier for the Connect provider dialog (`opencode_kernel::OverlayStack`).
pub const OVERLAY_ID: &str = "dialog.connect";
/// Overlay stack identifier for the ChatGPT plan welcome dialog (`opencode_kernel::OverlayStack`).
pub const WELCOME_OVERLAY_ID: &str = "dialog.connect.welcome";

/// `providers/connect/remote.tsx::authServerName`. An HTTP connection on *any* loopback
/// port counts as local here. A plugin's SSH tunnel remains remote even when it listens on
/// loopback. Do not use this UI distinction to grant access to local files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AuthHost {
    #[default]
    Local,
    Remote(String),
}

/// The open dialog and what its inputs hold.
pub struct Open {
    pub dialog: Dialog,
    /// Where the credential is stored, not whether this endpoint shares a filesystem.
    pub auth_host: AuthHost,
    /// The folder the sign-in belongs to; none for the server's default location.
    pub dir: Option<String>,
    /// The composer whose model Continue sets; none starts a new session with it.
    pub scope: Option<String>,
    pub search: TextEdit,
    /// The list's lit row, by integration id.
    pub active: Option<String>,
    /// The API key or the pasted code; a new step starts it empty.
    pub entry: TextEdit,
    /// A form step's text, or its own answer; a new step starts it on its default.
    pub field: TextEdit,
    /// The account whose options are open.
    pub menu: Option<String>,
    pub rename: TextEdit,
}

/// Active modal owned by [`Connect`]: the provider connect dialog and the ChatGPT plan welcome
/// card are mutually exclusive.
#[derive(Default)]
pub enum Modal {
    #[default]
    Closed,
    Dialog(Box<Open>),
    Welcome,
}

pub enum ModalEvent {
    /// Open (or reopen) the Connect provider dialog, replacing any active modal.
    OpenDialog(Box<Open>),
    /// Close the Connect provider dialog; shows the ChatGPT plan welcome when `show_welcome` is true.
    CloseDialog { show_welcome: bool },
    /// Open the ChatGPT plan welcome modal directly when no modal is open.
    OpenWelcome,
    /// Close the ChatGPT plan welcome modal.
    CloseWelcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalEffect {
    OpenOverlay(&'static str),
    CloseOverlay(&'static str),
    MarkWelcomeSeen,
    FocusComposer,
}

impl Modal {
    pub fn step(&mut self, event: ModalEvent) -> Effects<ModalEffect> {
        match (&*self, event) {
            (Self::Closed | Self::Dialog(_), ModalEvent::OpenDialog(open)) => {
                *self = Self::Dialog(open);
                Effects::one(ModalEffect::OpenOverlay(OVERLAY_ID))
            }
            (Self::Welcome, ModalEvent::OpenDialog(open)) => {
                *self = Self::Dialog(open);
                Effects::two(ModalEffect::CloseOverlay(WELCOME_OVERLAY_ID), ModalEffect::OpenOverlay(OVERLAY_ID))
            }
            (Self::Dialog(_), ModalEvent::CloseDialog { show_welcome: true }) => {
                *self = Self::Welcome;
                let mut effects = Effects::two(ModalEffect::CloseOverlay(OVERLAY_ID), ModalEffect::MarkWelcomeSeen);
                effects.push(ModalEffect::OpenOverlay(WELCOME_OVERLAY_ID));
                effects
            }
            (Self::Dialog(_), ModalEvent::CloseDialog { show_welcome: false }) => {
                *self = Self::Closed;
                Effects::two(ModalEffect::CloseOverlay(OVERLAY_ID), ModalEffect::FocusComposer)
            }
            (Self::Closed | Self::Welcome, ModalEvent::CloseDialog { .. }) => Effects::none(),
            (Self::Closed, ModalEvent::OpenWelcome) => {
                *self = Self::Welcome;
                Effects::two(ModalEffect::MarkWelcomeSeen, ModalEffect::OpenOverlay(WELCOME_OVERLAY_ID))
            }
            (Self::Dialog(_) | Self::Welcome, ModalEvent::OpenWelcome) => Effects::none(),
            (Self::Welcome, ModalEvent::CloseWelcome) => {
                *self = Self::Closed;
                Effects::two(ModalEffect::CloseOverlay(WELCOME_OVERLAY_ID), ModalEffect::FocusComposer)
            }
            (Self::Closed | Self::Dialog(_), ModalEvent::CloseWelcome) => Effects::none(),
        }
    }
}

#[derive(Default)]
pub struct Connect {
    pub modal: Modal,
    /// Fallback editor targets so `on_edit` callbacks never panic if fired after close.
    scratch_search: TextEdit,
    scratch_entry: TextEdit,
    scratch_field: TextEdit,
    scratch_rename: TextEdit,
}

impl Connect {
    /// Whether the Connect provider dialog is open.
    pub fn is_open(&self) -> bool {
        matches!(self.modal, Modal::Dialog(_))
    }

    /// Whether the ChatGPT plan welcome modal is open.
    pub fn is_welcome_open(&self) -> bool {
        matches!(self.modal, Modal::Welcome)
    }

    /// Whether either Connect modal (provider dialog or ChatGPT plan welcome) is open.
    pub fn is_modal_open(&self) -> bool {
        !matches!(self.modal, Modal::Closed)
    }

    pub fn dialog(&self) -> Option<&Open> {
        match &self.modal {
            Modal::Dialog(open) => Some(open),
            Modal::Closed | Modal::Welcome => None,
        }
    }

    pub fn dialog_mut(&mut self) -> Option<&mut Open> {
        match &mut self.modal {
            Modal::Dialog(open) => Some(open),
            Modal::Closed | Modal::Welcome => None,
        }
    }

    pub(crate) fn search_mut(&mut self) -> &mut TextEdit {
        match &mut self.modal {
            Modal::Dialog(o) => &mut o.search,
            Modal::Closed | Modal::Welcome => &mut self.scratch_search,
        }
    }

    pub(crate) fn entry_mut(&mut self) -> &mut TextEdit {
        match &mut self.modal {
            Modal::Dialog(o) => &mut o.entry,
            Modal::Closed | Modal::Welcome => &mut self.scratch_entry,
        }
    }

    pub(crate) fn field_mut(&mut self) -> &mut TextEdit {
        match &mut self.modal {
            Modal::Dialog(o) => &mut o.field,
            Modal::Closed | Modal::Welcome => &mut self.scratch_field,
        }
    }

    pub(crate) fn rename_mut(&mut self) -> &mut TextEdit {
        match &mut self.modal {
            Modal::Dialog(o) => &mut o.rename,
            Modal::Closed | Modal::Welcome => &mut self.scratch_rename,
        }
    }
}

/// Object-safe host boundary for the Connect provider dialog, ChatGPT plan welcome, integration
/// catalogs, and server-wide picker catalog reads.
pub trait ConnectHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback + opencode_kernel::AppOverlays {
    fn connect(&self) -> &Connect;
    fn connect_mut(&mut self) -> &mut Connect;
    fn catalogs(&self) -> &Catalogs;
    fn catalogs_mut(&mut self) -> &mut Catalogs;

    /// Folder for a composer-launched sign-in (the active session's or draft's directory).
    fn active_directory(&self) -> String;
    /// Picker scope key for the composer that opened the dialog (e.g. session ID or `"draft"`).
    fn picker_scope(&self) -> String {
        "draft".into()
    }
    /// Whether the active server stores credentials locally or on a named remote server.
    fn auth_host(&self) -> AuthHost {
        AuthHost::Local
    }
    /// Stable window identifier passed as Console OAuth `return_window`, or empty if none.
    fn window_id(&self) -> &str {
        ""
    }

    /// Whether this window may mutate server state (defaults to [`opencode_kernel::AppEnv::is_live`]).
    fn can_write(&self) -> bool {
        self.is_live()
    }

    /// Whether the ChatGPT plan welcome card has already been shown.
    fn chatgpt_welcome_seen(&self) -> bool {
        false
    }
    /// Persists that the ChatGPT plan welcome card has been shown.
    fn mark_chatgpt_welcome_seen(&mut self) {}

    /// Models currently known to the composer's pickers.
    fn models(&self) -> &[Model];
    /// Updates the host's picker providers list from `/api/provider`.
    fn load_providers(&mut self, list: &Value);
    /// Updates the host's picker config from `/api/config`.
    fn load_config(&mut self, list: &Value);
    /// Updates the host's picker models list from `/api/model`.
    fn load_models(&mut self, list: &Value);
    /// Updates the host's picker agents list from `/api/agent`.
    fn load_agents(&mut self, list: &Value);
    /// Marks newly unlocked model keys visible in model preferences.
    fn reveal_models(&mut self, keys: &[String]);
    /// Applies the model picked on the final Models step (into `scope`, or starts a new session
    /// when `scope` is `None`).
    fn apply_connected_model(&mut self, key: &str, scope: Option<&str>, tx: &mut Tx);
    /// Notified after catalog/model data changes (optional hook for host picker/UI refreshes).
    fn on_models_changed(&mut self, tx: &mut Tx) {
        let _ = tx;
    }

    /// Advances and returns the server-scoped generation token for [`picker_reads`].
    fn next_picker_reads_generation(&mut self) -> Attempt;
    /// Current server-scoped generation token for [`picker_reads`].
    fn current_picker_reads_generation(&self) -> Attempt;

    /// Prepares the shell before opening the Connect dialog (e.g. closes pickers/manage models
    /// and blurs composer).
    fn before_dialog_open(&mut self) {}
    /// Restores composer focus after closing the Connect or Welcome dialog when appropriate.
    fn auto_focus_composer(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    /// Opens `url` in the system browser; returns `true` when launch succeeded.
    fn open_external(&mut self, url: &str, tx: &mut Tx) -> bool {
        tx.open_url(url).is_ok()
    }
    /// Schedules an off-thread server request, timer, or next-poll deferred reply.
    fn spawn_job(&mut self, job: ConnectJob);
}

fn focused(text: &str) -> TextEdit {
    let mut t = TextEdit::new(text);
    t.focused = true;
    t
}

pub(crate) fn apply_modal(host: &mut dyn ConnectHost, event: ModalEvent, tx: &mut Tx) {
    let effects = host.connect_mut().modal.step(event);
    for effect in effects {
        match effect {
            ModalEffect::OpenOverlay(id) => {
                let return_focus = tx.focused().map(str::to_owned);
                host.on_overlay_opened(id, return_focus);
            }
            ModalEffect::CloseOverlay(id) => host.on_overlay_closed(id),
            ModalEffect::MarkWelcomeSeen => host.mark_chatgpt_welcome_seen(),
            ModalEffect::FocusComposer => host.auto_focus_composer(tx),
        }
    }
}

/// Opens the dialog on its list, or on `provider`'s sign-in. From Settings it signs in at the
/// server's default location; from the composer, in the composer's folder.
pub fn open(host: &mut dyn ConnectHost, provider: Option<&str>, from_settings: bool, tx: &mut Tx) {
    tx.instant();
    host.before_dialog_open();
    let dir = (!from_settings).then(|| host.active_directory()).filter(|d| !d.is_empty());
    let scope = (!from_settings).then(|| host.picker_scope());
    let auth_host = host.auth_host();
    apply_modal(
        host,
        ModalEvent::OpenDialog(Box::new(Open {
            dialog: Dialog::default(),
            auth_host,
            dir: dir.clone(),
            scope,
            search: focused(""),
            active: None,
            entry: TextEdit::new(""),
            field: TextEdit::new(""),
            menu: None,
            rename: TextEdit::new(""),
        })),
        tx,
    );
    fresh_inputs(host);
    catalog::sync_catalogs(host, dir);
    if let Some(id) = provider {
        let accounts = accounts_of(host, id);
        connect_event(host, Event::Selected { provider: id.to_owned(), from_picker: false, accounts }, tx);
    }
}

/// Closes the Connect dialog, notifies `OverlayStack`, and opens the ChatGPT plan welcome when
/// earned or restores composer focus.
pub fn close(host: &mut dyn ConnectHost, tx: &mut Tx) {
    if !host.connect().is_open() {
        return;
    }
    tx.instant();
    let show_welcome = host.connect().dialog().is_some_and(|o| o.dialog.chatgpt_welcome) && !host.chatgpt_welcome_seen();
    connect_event(host, Event::Disposed, tx);
    apply_modal(host, ModalEvent::CloseDialog { show_welcome }, tx);
}

/// Opens the Connect dialog directly onto a synthetic `provider` + `step` (for scene previews).
pub fn preview_step(host: &mut dyn ConnectHost, provider: &str, step: Step) {
    open(host, None, false, &mut Tx::default());
    if let Some(open) = host.connect_mut().dialog_mut() {
        open.dialog = Dialog {
            session: 1,
            provider: Some(Provider { id: provider.into(), accounts: Accounts::default(), step, closed: false }),
            ..Dialog::default()
        };
        open.search.focused = false;
    }
}

/// The folder the currently open Connect dialog targets, if any.
pub fn connect_dir(host: &dyn ConnectHost) -> Option<&str> {
    host.connect().dialog().and_then(|o| o.dir.as_deref())
}

/// The integration's accounts as the server lists them, the active one first.
pub fn accounts_of(host: &dyn ConnectHost, id: &str) -> Accounts {
    let Some(i) = catalog::integration_of(host, connect_dir(host), id) else { return Accounts::default() };
    let list = i.accounts();
    Accounts {
        count: list.len(),
        active: list.first().map(|a| a.id.clone()),
        problem: list.first().is_some_and(|a| a.status.is_some()),
    }
}

/// Runs one event through the dialog and its effects.
pub fn connect_event(host: &mut dyn ConnectHost, event: Event, tx: &mut Tx) {
    let (effects, moved, scope) = {
        let Some(open) = host.connect_mut().dialog_mut() else { return };
        let scope = open.scope.clone();
        let (next, effects) = dialog::transition(&open.dialog, event);
        // A transition to another step (or method, or form) starts its inputs afresh.
        let moved = steps::inputs_id(&open.dialog) != steps::inputs_id(&next);
        open.dialog = next;
        (effects, moved, scope)
    };
    if moved {
        fresh_inputs(host);
        // Like the dialog's focus host. A freshly focused entry or the picker search wins.
        if host.connect().dialog().is_some_and(|o| o.dialog.provider.is_some()) {
            tx.focus("connect-focus-host", false);
        }
    }
    for effect in effects {
        run::run_effect(host, effect, scope.as_deref(), tx);
    }
}

/// A new step's inputs start empty, its first one taking the keys.
fn fresh_inputs(host: &mut dyn ConnectHost) {
    let Some(open) = host.connect_mut().dialog_mut() else { return };
    let value = steps::field_default(&open.dialog);
    open.field = focused(&value);
    open.entry = focused("");
    open.search.focused = open.dialog.provider.is_none();
    open.menu = None;
}

/// The catalogs changed: the dialog follows the accounts and Claude Agent's login.
pub fn catalogs_changed(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let Some((id, claude)) = host
        .connect()
        .dialog()
        .and_then(|o| o.dialog.provider.as_ref())
        .map(|p| (p.id.clone(), matches!(p.step, Step::Claude { .. })))
    else {
        return;
    };
    let accounts = accounts_of(host, &id);
    connect_event(host, Event::AccountsSynced(accounts.clone()), tx);
    if claude {
        let models = !connection_models(host, &id).is_empty();
        connect_event(host, Event::ClaudeStatus { problem: accounts.problem, models }, tx);
    }
}

/// The models a connection to `id` offers: its providers' (`id` or integrating as `id`).
pub fn connection_models(host: &dyn ConnectHost, id: &str) -> Vec<Model> {
    let infos = catalog::provider_infos(host, connect_dir(host)).unwrap_or_default();
    let of = |provider: &str| provider == id || infos.iter().any(|p| p.id == provider && p.integration.as_deref() == Some(id));
    host.models().iter().filter(|m| of(&m.provider)).cloned().collect()
}

/// The first model the connection offers, `provider:id`, as its models step lights it.
pub fn first_model(host: &dyn ConnectHost, provider: &str) -> Option<String> {
    steps::connection_groups(host, provider).first().and_then(|(_, models)| models.first()).map(|m| m.key.clone())
}

/// Display name for integration/provider `id` in the open dialog's location.
pub fn provider_name(host: &dyn ConnectHost, id: &str) -> String {
    let dir = connect_dir(host);
    catalog::integration_of(host, dir, id)
        .map(|i| i.name.clone())
        .or_else(|| catalog::provider_infos(host, dir).and_then(|ps| ps.iter().find(|p| p.id == id).map(|p| p.name.clone())))
        .unwrap_or_else(|| id.to_owned())
}

/// The dialog's keys: Escape closes it (or its open account menu), and the list's arrows and
/// Enter pick a provider.
pub fn key(host: &mut dyn ConnectHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if welcome::key(host, ev, tx) {
        return true;
    }
    let (menu_open, in_picker) = match host.connect().dialog() {
        Some(open) => (open.menu.is_some(), open.dialog.provider.is_none()),
        None => return false,
    };
    if ev.key == Key::Escape {
        if menu_open {
            tx.instant();
            if let Some(open) = host.connect_mut().dialog_mut() {
                open.menu = None;
            }
            return true;
        }
        close(host, tx);
        return true;
    }
    if in_picker {
        return view::picker_key(host, ev, tx);
    }
    false
}
