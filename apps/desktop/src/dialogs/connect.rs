//! Desktop host adapter for `opencode-feature-connect` (`features/connect`).

pub use opencode_feature_connect::*;

use opencode_kernel::{Feature, KernelBuilder};
use opencode_kit::integration::Integration;
use rune::Tx;
use rune::machine::Attempt;
use serde_json::Value;
use session_view::pickers::Model;

use crate::App;
use crate::jobs::Apply;
use crate::pickers::PickerHost as _;

impl ConnectHost for App {
    fn connect(&self) -> &Connect {
        &self.connect
    }

    fn connect_mut(&mut self) -> &mut Connect {
        &mut self.connect
    }

    fn catalogs(&self) -> &Catalogs {
        &self.catalogs
    }

    fn catalogs_mut(&mut self) -> &mut Catalogs {
        &mut self.catalogs
    }

    fn active_directory(&self) -> String {
        match &*self.route {
            crate::Route::Session { id, .. } => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir().to_owned(),
        }
    }

    fn picker_scope(&self) -> String {
        crate::pickers::PickerHost::picker_scope(self)
    }

    fn auth_host(&self) -> AuthHost {
        use opencode_kit::servers::{ServerConn, registry::loopback_origin};
        let key = crate::servers::runtime::active(self);
        if key.is_sidecar() {
            return AuthHost::Local;
        }
        let Some(entry) = crate::servers::registry::list(self).into_iter().find(|e| &e.key() == key) else {
            return AuthHost::Remote(crate::servers::runtime::label(key));
        };
        match &entry.conn {
            ServerConn::Sidecar { .. } => AuthHost::Local,
            ServerConn::Http { http, .. } if loopback_origin(&http.url).is_some() => AuthHost::Local,
            conn => AuthHost::Remote(crate::servers::registry::label(conn)),
        }
    }

    fn window_id(&self) -> &str {
        &self.window.id
    }

    fn can_write(&self) -> bool {
        App::can_write(self)
    }

    fn chatgpt_welcome_seen(&self) -> bool {
        self.settings.chatgpt_welcome_seen
    }

    fn mark_chatgpt_welcome_seen(&mut self) {
        self.change_settings(|x| x.chatgpt_welcome_seen = true);
    }

    fn models(&self) -> &[Model] {
        &self.pickers.models
    }

    fn load_providers(&mut self, list: &Value) {
        self.pickers.load_providers(list);
    }

    fn load_config(&mut self, list: &Value) {
        self.pickers.load_config(list);
    }

    fn load_models(&mut self, list: &Value) {
        self.pickers.load_models(list);
    }

    fn load_agents(&mut self, list: &Value) {
        self.pickers.load_agents(list);
    }

    fn reveal_models(&mut self, keys: &[String]) {
        self.pickers.prefs.set_visibility(keys.iter().map(String::as_str), true);
    }

    fn apply_connected_model(&mut self, key: &str, scope: Option<&str>, tx: &mut Tx) {
        let Some(m) = self.pickers.models.iter().find(|m| m.key == key).cloned() else { return };
        if scope.is_none() {
            self.close_settings();
            self.new_session(tx);
        }
        let scope = scope.map(str::to_owned).unwrap_or_else(|| crate::pickers::PickerHost::picker_scope(self));
        let mut selection = self.selection();
        selection.model = opencode_kit::model::ModelRef { id: m.id, provider: m.provider, variant: String::new() };
        self.pickers.prefs.picked(key);
        self.pickers.selections.insert(scope, selection);
    }

    fn next_picker_reads_generation(&mut self) -> Attempt {
        self.jobs.picker_reads.next()
    }

    fn current_picker_reads_generation(&self) -> Attempt {
        self.jobs.picker_reads.current()
    }

    fn before_dialog_open(&mut self) {
        self.close_picker(false);
        self.pickers.close_manage();
        self.composer.focused = false;
    }

    fn auto_focus_composer(&mut self, tx: &mut Tx) {
        if crate::auto_focus_composer(tx) {
            self.composer.focused = self.has_composer() && !self.settings.open;
        }
    }

    fn open_external(&mut self, url: &str, tx: &mut Tx) -> bool {
        crate::open_in::open_external(tx, url)
    }

    fn spawn_job(&mut self, job: ConnectJob) {
        match job {
            ConnectJob::Server { delay, request } => {
                self.job_after(delay, move |srv| {
                    let reply = execute_request(srv, request);
                    Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
                });
            }
            ConnectJob::Timer { delay, reply } => {
                self.job_after(delay, move |_| {
                    Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply
                });
            }
            ConnectJob::Defer { reply } => {
                self.later(Box::new(move |s: &mut App, tx: &mut Tx| apply_reply(s, reply, tx)) as Apply);
            }
        }
    }
}

impl App {
    /// Opens the dialog on its list, or on `provider`'s sign-in. From Settings it signs in at the
    /// server's default location; from the composer, in the composer's folder.
    pub fn open_connect(&mut self, provider: Option<&str>, from_settings: bool, tx: &mut Tx) {
        open(self, provider, from_settings, tx);
    }

    pub fn close_connect(&mut self, tx: &mut Tx) {
        close(self, tx);
    }

    /// The integrations at `dir` (the server's default when none), once read.
    pub(crate) fn integrations(&self, dir: Option<&str>) -> Option<&[Integration]> {
        integrations(self, dir)
    }

    #[allow(dead_code)]
    pub(crate) fn integration_of(&self, dir: Option<&str>, id: &str) -> Option<&Integration> {
        integration_of(self, dir, id)
    }

    /// The providers the server runs at `dir`, once read.
    pub(crate) fn provider_infos(&self, dir: Option<&str>) -> Option<&[ProviderInfo]> {
        provider_infos(self, dir)
    }

    /// Puts a scene's integrations and providers in the server's default slot, as a sync would.
    pub(crate) fn seed_catalogs(&mut self, integrations: Vec<Integration>, providers: Vec<ProviderInfo>) {
        seed_catalogs(self, integrations, providers);
    }

    /// Reads `dir`'s integrations, providers, and model list again.
    pub(crate) fn sync_catalogs(&mut self, dir: Option<String>) {
        sync_catalogs(self, dir);
    }
}

/// Kernel feature registration for the Connect provider and ChatGPT plan welcome dialogs.
pub struct ConnectFeature;

impl Feature<App> for ConnectFeature {
    fn id(&self) -> &'static str {
        "feature.connect"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_overlay_dismiss(OVERLAY_ID, |s, tx| close(s, tx));
        builder.register_overlay_dismiss(WELCOME_OVERLAY_ID, |s, tx| welcome::close(s, tx));
    }
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("provider.connect", opencode_i18n::key!("command.provider.connect"), |s, tx| s.open_connect(None, false, tx))
        .describe(opencode_i18n::key!("command.provider.connect.description"))
        .slash("connect")
        .when(App::has_composer),
]);

mod scenes {
    //! The connect dialog's steps that only a write reaches, as scenes (`--scene connect-models`):
    //! a run that can't write still shows them. Each sets the step as the simulated server leaves
    //! it: Simulated connected with its models, GitHub Copilot's device sign-in refused by the
    //! simulated network, and Poe's browser sign-in waiting. The `-long` ones fill a step past the
    //! dialog's height: 25 models, and an error many lines long.

    use std::collections::HashSet;
    use std::rc::Rc;

    use opencode_kit::integration::{Authorization, Method, MethodKind};

    use super::dialog::{ReturnTo, Step};
    use super::machine::{Catalog, Paste, Retry, Sel, Signin, State};
    use crate::App;

    fn oauth(id: &str, label: &str) -> Method {
        Method { kind: MethodKind::Oauth, id: Some(id.into()), label: Some(label.into()), form: vec![] }
    }

    fn sel(method: Method) -> Sel {
        Sel { attempt: 2, catalog: Rc::new(Catalog { integration: None, methods: vec![method] }), method: 0, answer: None }
    }

    fn connect(state: State) -> Step {
        Step::Connect { back: ReturnTo::Picker, copied: None, connect: state }
    }

    /// 25 models from Drive, the first of them picked.
    fn long_models(s: &mut App) {
        s.pickers.models.retain(|m| m.provider != "drive");
        s.pickers.models.extend((1..=25).map(|i| crate::pickers::Model {
            key: format!("drive:model-{i}"),
            id: format!("model-{i}"),
            provider: "drive".into(),
            group: "Drive".into(),
            name: format!("Drive Model {i}"),
            variants: vec![],
            hidden: false,
            free: false,
            latest: false,
            inputs: vec![],
            tools: true,
        }));
        super::preview_step(s, "drive", Step::Models { selected: "drive:model-1".into(), collapsed: HashSet::new() });
    }

    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
        ("connect-models", |s| super::preview_step(s, "drive", Step::Models { selected: "drive:simulated".into(), collapsed: HashSet::new() })),
        ("connect-models-long", long_models),
        ("connect-failed-long", |s| {
            let error = (1..=30).map(|i| format!("at frame {i}: ECONNREFUSED 127.0.0.1:{}", 4000 + i)).collect::<Vec<_>>().join("\n");
            super::preview_step(s, "github-copilot", connect(State::Failed(sel(oauth("device", "Login with GitHub Copilot")), error, Retry::Start)));
        }),
        ("connect-failed", |s| {
            let error = "ECONNREFUSED: Unable to connect. Is the computer able to access the url?".to_owned();
            super::preview_step(s, "github-copilot", connect(State::Failed(sel(oauth("device", "Login with GitHub Copilot")), error, Retry::Start)));
        }),
        ("connect-browser", |s| {
            let method = oauth("poe", "Login with Poe (browser)");
            let auth = Authorization {
                attempt: "con_parity".into(),
                url: "https://poe.com/oauth/authorize?response_type=code&scope=apikey%3Acreate".into(),
                instructions: "Complete authorization in your browser. This window will close automatically.".into(),
                code: false,
                paste: false,
                created: 0,
                expires: 0,
            };
            super::preview_step(
                s,
                "poe",
                connect(State::Waiting(sel(method.clone()), Signin::Browser { method, auth, failed: false, paste: Paste::Unsupported })),
            );
        }),
    ]);
}
