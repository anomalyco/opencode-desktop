//! The window's side of Add WSL server (`dialog-machine.ts`, `probe-machine.ts`, `probes.ts`):
//! which view shows, the call to the main side it waits for (claimed on the click, so the dialog
//! is busy before the main side's state says so), and the probes it runs for what it shows, one
//! at a time. A probe that failed runs again only once the user asks to check again.

use std::collections::BTreeSet;

use opencode_plugin::prelude::*;

use crate::controller::{CoreRef, with};
use crate::model::{self, Plan};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Main,
    Catalog,
}

/// A call to the main side the dialog waits for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    InstallWsl,
    RefreshDistros,
    InstallOpencode(String),
    AddServer(String),
    InstallDistro(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Status {
    #[default]
    Main,
    MainRunning { attempt: u64, op: Op },
    Catalog { target: Option<String> },
    CatalogInstalling { attempt: u64, target: Option<String>, distro: String },
    /// A server was added: the dialog stays busy while it closes.
    Completed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DialogState {
    pub status: Status,
    pub issued: u64,
    pub selected: Option<String>,
}

impl DialogState {
    pub fn view(&self) -> View {
        if matches!(self.status, Status::Catalog { .. } | Status::CatalogInstalling { .. }) { View::Catalog } else { View::Main }
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn catalog_target(&self) -> Option<String> {
        match &self.status {
            Status::Catalog { target } | Status::CatalogInstalling { target, .. } => target.clone(),
            _ => None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.status, Status::MainRunning { .. } | Status::CatalogInstalling { .. } | Status::Completed)
    }

    pub fn adding(&self) -> bool {
        matches!(&self.status, Status::MainRunning { op: Op::AddServer(_), .. } | Status::Completed)
    }

    pub fn installing_catalog(&self) -> bool {
        matches!(self.status, Status::CatalogInstalling { .. })
    }

    pub fn installing_opencode(&self, distro: Option<&str>) -> bool {
        matches!(&self.status, Status::MainRunning { op: Op::InstallOpencode(d), .. } if Some(d.as_str()) == distro)
    }
}

/// The probes (`probe-machine.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Probes {
    issued: u64,
    /// The probe running now: its attempt and plan.
    running: Option<(u64, String, bool)>,
    /// The key whose probe last succeeded, held until a plan moves past it.
    completed: Option<String>,
    failed: BTreeSet<String>,
}

impl Probes {
    /// Whether a probe of the distros the dialog can add runs (their rows say Checking).
    pub fn probing_addable(&self) -> bool {
        self.running.as_ref().is_some_and(|r| r.2)
    }
}

/// The dialog's window state, beside the controller it calls.
pub struct Ui {
    pub dialog: DialogState,
    pub probes: Probes,
    /// The catalog's search field.
    pub search: EditorHandle,
}

pub type UiRef = std::rc::Rc<std::cell::RefCell<Ui>>;

/// Opens a fresh dialog.
pub fn reset(ui: &UiRef) {
    let mut u = ui.borrow_mut();
    u.dialog = DialogState::default();
    u.probes = Probes::default();
}

/// A user action: changes the dialog, starts its call, and plans the next probe.
pub fn act(ui: &UiRef, core: &CoreRef, cx: &mut ActionCx, search: &str, f: impl FnOnce(&mut DialogState, &model::Model) -> Option<Op>) {
    let m = model_now(ui, core, search);
    let op = f(&mut ui.borrow_mut().dialog, &m);
    if let Some(op) = op {
        start(ui, core, cx, op);
    }
    sync(ui, core, cx, search);
}

fn model_now(ui: &UiRef, core: &CoreRef, search: &str) -> model::Model {
    let ui = ui.borrow();
    let core = core.borrow();
    model::build(Some(&core.published), &ui.dialog, search, ui.probes.probing_addable())
}

/// Starts `op`, claiming the dialog first.
fn start(ui: &UiRef, core: &CoreRef, cx: &mut ActionCx, op: Op) {
    let attempt = {
        let mut u = ui.borrow_mut();
        u.dialog.issued += 1;
        let attempt = u.dialog.issued;
        u.dialog.status = match &op {
            Op::InstallDistro(distro) => Status::CatalogInstalling { attempt, target: Some(distro.clone()), distro: distro.clone() },
            _ => Status::MainRunning { attempt, op: op.clone() },
        };
        if matches!(op, Op::RefreshDistros | Op::InstallDistro(_)) {
            u.probes.failed.clear();
        }
        attempt
    };
    let (ui2, core2) = (ui.clone(), core.clone());
    let done: crate::controller::Done = Box::new(move |cx, result| settle(&ui2, &core2, cx, attempt, result));
    with(core, cx, |c, cx| match &op {
        Op::InstallWsl => c.install_wsl(cx, done),
        Op::RefreshDistros => c.refresh_distros(cx, done),
        Op::InstallOpencode(d) => c.install_opencode(cx, d, done),
        Op::AddServer(d) => c.add_server(cx, d, done),
        Op::InstallDistro(d) => c.install_distro(cx, d, done),
    });
}

/// A call ended: an installed distro returns to the list, a failed install stays in the
/// catalog, an added server closes the dialog.
fn settle(ui: &UiRef, core: &CoreRef, cx: &mut ActionCx, attempt: u64, result: Result<(), String>) {
    let close = {
        let mut u = ui.borrow_mut();
        let selected = u.dialog.selected.clone();
        let (settled, add) = match &u.dialog.status {
            Status::MainRunning { attempt: a, op } if *a == attempt => (Status::Main, matches!(op, Op::AddServer(_))),
            Status::CatalogInstalling { attempt: a, target, .. } if *a == attempt => (if result.is_ok() { Status::Main } else { Status::Catalog { target: target.clone() } }, false),
            _ => return,
        };
        u.dialog.status = if result.is_ok() && add { Status::Completed } else { settled };
        u.dialog.selected = selected;
        result.is_ok() && add
    };
    if let Err(e) = result {
        cx.toast_error("Request failed", &e);
    }
    if close {
        cx.close_dialog();
        return;
    }
    let search = cx.editor_text(ui.borrow().search);
    sync(ui, core, cx, &search);
}

/// Runs the probe the dialog needs next, if it isn't running or held (`probe-machine.ts`).
pub fn sync(ui: &UiRef, core: &CoreRef, cx: &mut ActionCx, search: &str) {
    let m = model_now(ui, core, search);
    let plan = {
        let core = core.borrow();
        model::plan(Some(&core.published), m.view, m.busy && !ui.borrow().probes.probing_addable(), m.selected.as_deref(), &m.addable)
    };
    let busy = m.busy && !ui.borrow().probes.probing_addable();
    let attempt = {
        let mut u = ui.borrow_mut();
        let p = &mut u.probes;
        if p.running.is_some() {
            return;
        }
        let key = plan.as_ref().map(Plan::key);
        let held = p.completed.clone().filter(|c| busy || key.as_deref() == Some(c.as_str()));
        p.completed = held.clone();
        let Some(plan) = plan.as_ref() else { return };
        let key = plan.key();
        if held.as_deref() == Some(key.as_str()) || p.failed.contains(&key) {
            return;
        }
        p.issued += 1;
        p.running = Some((p.issued, key, matches!(plan, Plan::Addable { .. })));
        p.issued
    };
    let Some(plan) = plan else { return };
    let (ui2, core2) = (ui.clone(), core.clone());
    let done: crate::controller::Done = Box::new(move |cx, result| {
        {
            let mut u = ui2.borrow_mut();
            let Some((a, key, _)) = u.probes.running.clone() else { return };
            if a != attempt {
                return;
            }
            u.probes.running = None;
            match &result {
                Ok(()) => u.probes.completed = Some(key),
                Err(_) => {
                    u.probes.failed.insert(key);
                }
            }
        }
        if let Err(e) = result {
            cx.toast_error("Request failed", &e);
        }
        let search = cx.editor_text(ui2.borrow().search);
        sync(&ui2, &core2, cx, &search);
    });
    with(core, cx, |c, cx| match plan {
        Plan::Runtime => c.probe_runtime(cx, done),
        Plan::Distros => c.refresh_distros(cx, done),
        Plan::Addable { distros, .. } => c.probe_addable(cx, &distros, done),
    });
}

/// The user asked to check again: probes that failed may run again.
pub fn clear_failures(ui: &UiRef) {
    ui.borrow_mut().probes.failed.clear();
}
