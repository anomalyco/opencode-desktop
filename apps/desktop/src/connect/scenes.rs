//! The connect dialog's steps that only a write reaches, as scenes (`--scene connect-models`):
//! a run that can't write still shows them. Each sets the step as the simulated server leaves
//! it: Simulated connected with its models, GitHub Copilot's device sign-in refused by the
//! simulated network, and Poe's browser sign-in waiting.

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

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("connect-models", |s| show(s, "drive", Step::Models { selected: "drive:simulated".into(), collapsed: HashSet::new() })),
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
        show(s, "poe", connect(State::Waiting(sel(method.clone()), Signin::Browser { method, auth, failed: false })));
    }),
]);
