//! Add WSL server, drawn (`dialog.tsx`, `dialog.css`): the installed distros to pick from with
//! each one's status, Check again, Need another distro? opening the catalog of distros to
//! install, and the setup card when WSL itself isn't there.

use opencode_plugin::abi::{BUTTON_CONTRAST, BUTTON_GHOST_MUTED, BUTTON_NEUTRAL, SHADOW_OVERLAY, SHADOW_RAISED};
use opencode_plugin::prelude::*;

use crate::WslWindow;
use crate::dialog::{self, Op, Status, View};
use crate::model::{self, Action, RuntimeState, Tone};

/// `Download` (the catalog card's icon) and the chevron after it, as `dialog.tsx` draws them.
const DOWNLOAD: &str = r#"<path d="M13.5564 10.4443V13.5554H4.22309C3.24087 13.5554 2.44531 13.5554 2.44531 13.5554V10.4443M11.112 5.99989L8.00087 9.111L4.88976 5.99989M8.00087 9.111L8.00087 2.44434" stroke="currentColor" fill="none"/>"#;
const CHEVRON: &str = r#"<path d="M6 12L10 8L6 4" stroke="currentColor" fill="none"/>"#;
const WARNING: &str = r#"<path fill-rule="evenodd" clip-rule="evenodd" d="M12 -0.00244141L23.6926 20.2498H0.308594L12 -0.00244141ZM12.7954 6.32932C12.5844 6.11834 12.2982 5.99982 11.9999 5.99982C11.7015 5.99982 11.4154 6.11834 11.2044 6.32932C10.9934 6.5403 10.8749 6.82645 10.8749 7.12482V11.6248C10.8749 11.9232 10.9934 12.2093 11.2044 12.4203C11.4154 12.6313 11.7015 12.7498 11.9999 12.7498C12.2982 12.7498 12.5844 12.6313 12.7954 12.4203C13.0064 12.2093 13.1249 11.9232 13.1249 11.6248V7.12482C13.1249 6.82645 13.0064 6.5403 12.7954 6.32932ZM13.0605 17.5605C12.7792 17.8418 12.3977 17.9998 11.9999 17.9998C11.6021 17.9998 11.2205 17.8418 10.9392 17.5605C10.6579 17.2792 10.4999 16.8976 10.4999 16.4998C10.4999 16.102 10.6579 15.7205 10.9392 15.4392C11.2205 15.1579 11.6021 14.9998 11.9999 14.9998C12.3977 14.9998 12.7792 15.1579 13.0605 15.4392C13.3418 15.7205 13.4999 16.102 13.4999 16.4998C13.4999 16.8976 13.3418 17.2792 13.0605 17.5605Z" fill="currentColor"/>"#;

pub fn intern_icons() {
    intern_icon("wsl-download", DOWNLOAD, 16.0);
    intern_icon("wsl-chevron", CHEVRON, 16.0);
    intern_icon("wsl-warning", WARNING, 24.0);
}

pub fn dialog(s: &WslWindow, cx: &mut ViewCx<WslWindow>, _: &Props) -> El {
    let Some(core) = s.core() else { return setup_card(cx, RuntimeState::Unavailable, Some("WSL is unavailable"), false, false) };
    let ui = s.ui.borrow();
    let search = cx.editor_text(ui.search);
    let version = (core.borrow().revision, ui.dialog.clone(), ui.probes.clone(), cx.editor_version(ui.search), cx.dark());
    let m = model::build(Some(&core.borrow().published), &ui.dialog, &search, ui.probes.probing_addable());
    let runtime_error = core.borrow().published.runtime.as_ref().and_then(|r| r.error.clone());
    let search_field = ui.search;
    drop(ui);
    cx.memo("dialog", version, move |cx| match m.runtime {
        RuntimeState::Ready => card(cx, &m, search_field),
        RuntimeState::Checking | RuntimeState::Loading => loading(cx),
        state => {
            let installable = runtime_error.as_deref().is_none_or(missing);
            setup_card(cx, state, runtime_error.as_deref(), installable, m.busy)
        }
    })
}

/// WSL isn't installed (the error names it, or there's none), rather than broken.
fn missing(error: &str) -> bool {
    let e = error.to_lowercase();
    e.contains("wsl is not installed") || e.contains("not been installed") || e.contains("wsl --install") || e.contains("wsl.exe --install")
}

/// `[data-slot="dialog-container"]` at 480: `layer-01`, the overlay elevation, 6 corners.
fn container(cx: &ViewCx<WslWindow>, radius: f32) -> El {
    let p = cx.pal();
    col().key("wsl-dialog").w(480.0).fill(p.layer1).radius(radius).shadow(SHADOW_OVERLAY).clip()
}

fn loading(cx: &mut ViewCx<WslWindow>) -> El {
    container(cx, 6.0).child(row().w_full().px(24.0).py(48.0).center().child(ui::loader(16.0)))
}

fn card(cx: &mut ViewCx<WslWindow>, m: &model::Model, search: EditorHandle) -> El {
    let p = cx.pal();
    let title = if m.view == View::Main { "Add WSL server" } else { "Install distro" };
    let header = row().w_full().padding(16.0).center_y().child(text(title).font(13.0, 530.0).line_height(16.0).tracking(-0.04).color(p.text));
    let divider = el().w_full().h(0.5).fill(p.border_strong).mt(0.0);
    let header = col().w_full().child(header).child(row().w_full().mt(0.0).child(divider));
    let (body, footer) = if m.view == View::Main { main_view(cx, m) } else { catalog_view(cx, m, search) };
    container(cx, 6.0).child(header).child(body).child(footer.px(16.0).pb(24.0).mt(8.0))
}

/// [data-slot="dialog-footer"]: its buttons at its end, 8 apart.
fn footer() -> El {
    row().w_full().gap(8.0).child(spacer())
}

fn main_view(cx: &mut ViewCx<WslWindow>, m: &model::Model) -> (El, El) {
    let p = cx.pal();
    let check_again = ui::button_with("Check again", BUTTON_GHOST_MUTED).small().disabled(m.busy).key("wsl-check-again").on_press(cx.handler(|s: &mut WslWindow, cx| s.check_again(cx)));
    let section = row()
        .w_full()
        .h(24.0)
        .pl(9.0)
        .gap(12.0)
        .center_y()
        .justify_between()
        .child(text("Installed distros").font(13.0, 440.0).line_height(16.0).tracking(-0.04).color(p.text))
        .child(check_again);
    let list = col().key("wsl-distro-list").w_full().max_h(220.0).radius(8.0).fill(p.bg_base).shadow(SHADOW_RAISED).scroll_y().clip();
    let list = if m.addable.is_empty() {
        let words = if m.visible_installed.is_empty() { "No distros detected yet." } else { "All installed distros are already added." };
        list.child(text(words).font(13.0, 440.0).line_height(18.2).color(p.muted).padding(16.0))
    } else {
        let n = m.addable.len();
        list.children(m.addable.iter().enumerate().map(|(i, d)| {
            let status = m.statuses.iter().find(|(name, _)| *name == d.name).and_then(|(_, s)| s.clone());
            let unsupported = d.version == Some(1);
            let tone = status.as_ref().map(|s| match s.tone {
                Tone::Success => cx.token("state-fg-success").unwrap_or(p.success),
                Tone::Warning => cx.token("state-fg-warning").unwrap_or(p.warning),
                Tone::Muted => p.muted,
            });
            let checked = m.selected.as_deref() == Some(d.name.as_str());
            let disabled = unsupported || m.busy;
            let label = status.map(|s| s.label).unwrap_or_default();
            radio_row(cx, &format!("wsl-distro-{}", d.name), &d.name, &label, tone, checked, disabled, i + 1 < n, 62.0, Pick::Distro(d.name.clone()))
        }))
    };
    let mut body = col().w_full().pt(16.0).px(16.0).pb(1.0).gap(16.0).child(section).child(list);
    if !m.installable.is_empty() {
        body = body.child(catalog_card(cx, m.busy));
    }
    let cancel = ui::button_with("Cancel", BUTTON_NEUTRAL).disabled(m.adding).key("wsl-cancel").on_press(cx.handler(|_: &mut WslWindow, cx| cx.close_dialog()));
    let pb = &m.primary;
    let mut primary = ui::button_with(pb.label.clone(), if pb.contrast { BUTTON_CONTRAST } else { BUTTON_NEUTRAL }).key("wsl-primary").loading(pb.loading).disabled(!pb.loading && pb.disabled);
    if let Some(w) = pb.width {
        primary = primary.w(w);
    }
    let (action, distro) = (pb.action, m.selected.clone());
    let primary = primary.on_press(cx.handler(move |s: &mut WslWindow, cx| {
        let Some(distro) = distro.clone() else { return };
        let op = match action {
            Some(Action::InstallOpencode) => Op::InstallOpencode(distro),
            Some(Action::Add) => Op::AddServer(distro),
            None => return,
        };
        s.act(cx, move |d, _| (d.status == Status::Main).then_some(op));
    }));
    (body, footer().child(cancel).child(primary))
}

#[derive(Clone)]
enum Pick {
    Distro(String),
    Catalog(String),
}

#[allow(clippy::too_many_arguments)]
fn radio_row(cx: &mut ViewCx<WslWindow>, key: &str, label: &str, description: &str, tone: Option<Rgba>, checked: bool, disabled: bool, rule: bool, min_h: f32, pick: Pick) -> El {
    let p = cx.pal();
    let mut r = row().key(key.to_owned()).w_full().min_h(min_h).pt(14.0).pr(14.0).pb(14.0).pl(16.0).role("radio").label(label.to_owned()).child(ui::radio(label.to_owned(), description.to_owned(), checked, tone).disabled(disabled).grow(1.0));
    if rule {
        r = col().w_full().child(r).child(el().w_full().h(0.5).fill(p.border));
    }
    if disabled {
        return r;
    }
    r.on_press(cx.handler(move |s: &mut WslWindow, cx| {
        let pick = pick.clone();
        s.act(cx, move |d, _| {
            match pick {
                Pick::Distro(name) if d.status == Status::Main => d.selected = Some(name),
                Pick::Catalog(name) => {
                    if let Status::Catalog { target } = &mut d.status {
                        *target = Some(name);
                    }
                }
                _ => {}
            }
            None
        });
    }))
}

/// "Need another distro?": opens the catalog.
fn catalog_card(cx: &mut ViewCx<WslWindow>, busy: bool) -> El {
    let p = cx.pal();
    let card = row()
        .key("wsl-catalog-card")
        .role("button")
        .label("Need another distro?")
        .w_full()
        .min_h(64.0)
        .pt(14.0)
        .pr(14.0)
        .pb(16.0)
        .pl(16.0)
        .gap(12.0)
        .radius(8.0)
        .border(0.5, p.border)
        .opacity(if busy { 0.5 } else { 1.0 })
        .child(icon("wsl-download", 16.0).color(p.icon))
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(6.0)
                .child(text("Need another distro?").font(13.0, 440.0).line_height(16.0).tracking(-0.04).color(p.text))
                .child(text("Install a Linux distribution from the WSL catalog").font(12.0, 440.0).line_height(12.0).tracking(0.05).color(p.muted)),
        )
        .child(icon("wsl-chevron", 16.0).color(p.icon_muted));
    if busy {
        return card;
    }
    card.on_press(cx.handler(|s: &mut WslWindow, cx| {
        s.act(cx, |d, m| {
            if d.status == Status::Main && !m.busy {
                d.status = Status::Catalog { target: m.installable.first().map(|i| i.name.clone()) };
            }
            None
        })
    }))
}

fn catalog_view(cx: &mut ViewCx<WslWindow>, m: &model::Model, search: EditorHandle) -> (El, El) {
    let p = cx.pal();
    let field = ui::field(search, "Search distros").large().disabled(m.busy).w_full().key("wsl-catalog-search");
    let n = m.filtered.len();
    let rows = m.filtered.iter().enumerate().map(|(i, d)| {
        let checked = m.catalog_target.as_deref() == Some(d.name.as_str());
        radio_row(cx, &format!("wsl-catalog-{}", d.name), &d.label, "", None, checked, m.busy, i + 1 < n, 48.0, Pick::Catalog(d.name.clone()))
    });
    let list = col().key("wsl-catalog-list").w_full().max_h(220.0).radius(8.0).border(0.5, p.border_strong).scroll_y().clip().children(rows.collect::<Vec<_>>());
    let body = col().w_full().pt(16.0).px(16.0).pb(1.0).gap(12.0).child(field).child(list);
    let cancel = ui::button_with("Cancel", BUTTON_NEUTRAL).disabled(m.busy).key("wsl-catalog-cancel").on_press(cx.handler(|s: &mut WslWindow, cx| {
        s.act(cx, |d, m| {
            if !m.busy {
                d.status = Status::Main;
            }
            None
        });
        let ui = s.ui.clone();
        dialog::clear_failures(&ui);
    }));
    let installing = m.installing_catalog;
    let target = m.catalog_target.clone();
    let install = ui::button_with("Install distro", if installing { BUTTON_NEUTRAL } else { BUTTON_CONTRAST })
        .loading(installing)
        .disabled(!installing && (m.busy || target.is_none()))
        .w(99.0)
        .key("wsl-catalog-install")
        .on_press(cx.handler(move |s: &mut WslWindow, cx| {
            let Some(distro) = target.clone() else { return };
            s.act(cx, move |d, m| (matches!(d.status, Status::Catalog { .. }) && !m.busy).then_some(Op::InstallDistro(distro)));
        }));
    (body, footer().child(cancel).child(install))
}

/// `DialogWslSetup`: WSL not installed, unavailable, or waiting on a restart.
fn setup_card(cx: &mut ViewCx<WslWindow>, state: RuntimeState, error: Option<&str>, installable: bool, busy: bool) -> El {
    let p = cx.pal();
    let (title, description) = match state {
        RuntimeState::PendingRestart => ("Windows needs a restart to finish installing WSL.", "Restart Windows to finish installing WSL, then reopen OpenCode."),
        _ if installable => ("WSL not installed", "WSL (Windows Subsystem for Linux) is required before OpenCode can add a WSL server"),
        _ => ("WSL unavailable", "OpenCode could not verify WSL on this machine."),
    };
    // dialog.tsx fills it #DBDBDB in either theme.
    let warning = [0.859, 0.859, 0.859, 1.0];
    let mut message = col()
        .w_full()
        .gap(8.0)
        .items_center()
        .child(icon("wsl-warning", 24.0).color(warning))
        .child(text(title).font(15.0, 530.0).line_height(20.0).tracking(-0.13).tabular().color(p.text))
        .child(text(description).font(13.0, 440.0).line_height(20.0).tracking(-0.04).tabular().max_w(300.0).wrap().color(p.muted).center());
    if let (false, Some(e)) = (installable, error) {
        message = message.child(text(e.to_owned()).font(12.0, 440.0).line_height(18.0).tracking(-0.04).max_w(360.0).wrap().mt(4.0).color(p.faint));
    }
    let mut content = col().key("wsl-setup").w_full().py(56.0).gap(24.0).items_center().child(message);
    if state == RuntimeState::Unavailable && installable {
        content = content.child(ui::button_with("Install WSL", BUTTON_NEUTRAL).disabled(busy).key("wsl-install").on_press(cx.handler(|s: &mut WslWindow, cx| s.act(cx, |d, _| (d.status == Status::Main).then_some(Op::InstallWsl)))));
    }
    if state != RuntimeState::Unavailable {
        content = content.child(ui::button_with("Close", BUTTON_NEUTRAL).key("wsl-close").on_press(cx.handler(|_: &mut WslWindow, cx| cx.close_dialog())));
    }
    col().key("wsl-dialog").w(480.0).min_h(264.0).fill(p.layer1).radius(10.0).shadow(SHADOW_OVERLAY).clip().center().child(content)
}
