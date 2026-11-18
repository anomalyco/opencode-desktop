//! The connect dialog's steps that only a write reaches, as scenes (`--scene connect-models`):
//! a run that can't write still shows them. Each sets the step as the simulated server leaves
//! it: Simulated connected with its models, GitHub Copilot's device sign-in refused by the
//! simulated network, and Poe's browser sign-in waiting. The `-long` ones fill a step past the
//! dialog's height: 25 models, and an error many lines long.

use std::collections::HashSet;
use std::rc::Rc;

use opencode_kit::integration::{Authorization, Method, MethodKind};
use rune::Tx;

use super::dialog::{Accounts, Dialog, Provider, ReturnTo, Step};
use super::machine::{Catalog, Retry, Sel, Signin, State};
use crate::App;

fn show(s: &mut App, provider: &str, step: Step) {
    s.open_connect(None, false, &mut Tx::default());
    if let Some(open) = s.connect.open.as_mut() {
        open.dialog = Dialog { session: 1, provider: Some(Provider { id: provider.into(), accounts: Accounts::default(), step, closed: false }), ..Dialog::default() };
        open.search.focused = false;
    }
}

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
    show(s, "drive", Step::Models { selected: "drive:model-1".into(), collapsed: HashSet::new() });
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("connect-models", |s| show(s, "drive", Step::Models { selected: "drive:simulated".into(), collapsed: HashSet::new() })),
    ("connect-models-long", long_models),
    ("connect-failed-long", |s| {
        let error = (1..=30).map(|i| format!("at frame {i}: ECONNREFUSED 127.0.0.1:{}", 4000 + i)).collect::<Vec<_>>().join("\n");
        show(s, "github-copilot", connect(State::Failed(sel(oauth("device", "Login with GitHub Copilot")), error, Retry::Start)));
    }),
    ("connect-failed", |s| {
        let error = "ECONNREFUSED: Unable to connect. Is the computer able to access the url?".to_owned();
        show(s, "github-copilot", connect(State::Failed(sel(oauth("device", "Login with GitHub Copilot")), error, Retry::Start)));
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
        show(s, "poe", connect(State::Waiting(sel(method.clone()), Signin::Browser { method, auth, failed: false, paste: super::machine::Paste::Unsupported })));
    }),
]);
