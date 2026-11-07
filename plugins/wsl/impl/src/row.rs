//! A WSL server's row in Settings › Servers (`row.tsx`): its health, the distro's name and the
//! WSL badge, the OpenCode version it runs, Install or Update OpenCode when it needs it, and its
//! menu (Retry start, Remove).

use opencode_plugin::abi::BUTTON_NEUTRAL;
use opencode_plugin::prelude::*;

use crate::WslWindow;
use crate::contract::{ServerRuntime, server_id_for_distro};
use crate::controller::with;
use crate::model;

pub fn server_row(s: &WslWindow, cx: &mut ViewCx<WslWindow>, props: &Props) -> El {
    let Some(core) = s.core() else { return cx.nothing() };
    let distro = props.id.clone();
    let core = core.borrow();
    let Some(item) = core.published.servers.iter().find(|i| i.config.distro == distro) else { return cx.nothing() };
    let view = model::row_view(&core.published, &distro);
    let health = opencode_plugin::system::server_health(&distro);
    let runtime = item.runtime.clone();
    let version = (core.revision, health.clone(), cx.dark());
    drop(core);
    cx.memo(&format!("row-{distro}"), version, move |cx| draw(cx, &distro, &runtime, health.is_some(), &view))
}

fn draw(cx: &mut ViewCx<WslWindow>, distro: &str, runtime: &ServerRuntime, healthy: bool, view: &model::RowView) -> El {
    let p = cx.pal();
    let name = row()
        .gap(4.0)
        .center_y()
        .min_w(0.0)
        .child(text(distro.to_owned()).font(13.0, 530.0).line_height(16.0).color(p.text).truncate().shrink())
        .child(row().px(4.0).py(2.0).radius(3.0).border(1.0, p.border).child(text("WSL").font(9.0, 440.0).line_height(9.0).color(p.muted)));
    let mut copy = col().grow(1.0).shrink().min_w(0.0).gap(6.0).child(name);
    copy = copy.child(text(view.version.as_ref().map(|v| format!("v{v}")).unwrap_or_default()).font(11.0, 440.0).line_height(11.0).color(p.muted));
    let lead = row().grow(1.0).shrink().min_w(0.0).gap(10.0).child(indicator(cx, runtime, healthy)).child(copy);
    let mut actions = row().gap(8.0).center_y();
    if let Some((label, disabled)) = &view.button {
        let name = distro.to_owned();
        actions = actions.child(ui::button_with(label.clone(), BUTTON_NEUTRAL).small().disabled(*disabled).key(format!("wsl-row-install-{distro}")).on_press(cx.handler(move |s: &mut WslWindow, cx| {
            let Some(core) = s.core() else { return };
            with(&core, cx, |c, cx| c.install_opencode(cx, &name, failure_toast()));
        })));
    }
    let anchor = format!("wsl-row-menu-{distro}");
    let (name, retry, removable) = (distro.to_owned(), view.retry, view.removable);
    let dots = el()
        .key(anchor.clone())
        .role("button")
        .label("More options")
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .instant_hover()
        .child(icon("outline-dots", 16.0).color(p.icon_muted))
        .on_press(cx.handler(move |_: &mut WslWindow, cx| {
            let mut menu = MenuSpec::new("WSL server").group("WSL server");
            if retry {
                menu = menu.item("retry", "Retry start");
            }
            let menu = menu.separator().item_with("remove", "Remove", removable, false).align_end();
            let name = name.clone();
            cx.open_menu::<WslWindow>(&anchor, &menu, move |s, cx, picked| match picked {
                Some("retry") => s.retry(cx, &name),
                Some("remove") => s.remove(cx, &name),
                _ => cx.unchanged(),
            });
        }));
    actions = actions.child(dots);
    row().key(format!("wsl-row-{distro}")).w_full().gap(16.0).center_y().child(lead).child(actions)
}

/// `ServerHealthIndicator`: a spinner while it starts, a green dot once it answers, red when it
/// failed, faint when stopped.
fn indicator(cx: &mut ViewCx<WslWindow>, runtime: &ServerRuntime, healthy: bool) -> El {
    let p = cx.pal();
    let dot = |c: Rgba| el().size(6.0, 6.0).my(3.5).radius(3.0).fill(c);
    match runtime {
        ServerRuntime::Starting => row().w(6.0).h(14.0).center().child(ui::loader(12.0)),
        ServerRuntime::Ready { .. } if healthy => dot(cx.token("icon-success-base").unwrap_or(p.success)),
        ServerRuntime::Ready { .. } => row().w(6.0).h(14.0).center().child(ui::loader(12.0)),
        ServerRuntime::Failed(_) => dot(cx.token("icon-critical-base").unwrap_or(p.danger)),
        ServerRuntime::Stopped => dot(p.icon_faint),
    }
}

/// A request's failure, as a toast; success says nothing (the row follows the state).
pub fn failure_toast() -> crate::controller::Done {
    Box::new(|cx, result| {
        if let Err(e) = result {
            cx.toast_error("Request failed", &e);
        }
    })
}

impl WslWindow {
    pub fn retry(&mut self, cx: &mut ActionCx, distro: &str) {
        let Some(core) = self.core() else { return cx.toast_error("Request failed", "WSL is unavailable") };
        with(&core, cx, |c, cx| c.start_server(cx, &server_id_for_distro(distro), failure_toast()));
    }

    pub fn remove(&mut self, cx: &mut ActionCx, distro: &str) {
        let Some(core) = self.core() else { return cx.toast_error("Request failed", "WSL is unavailable") };
        with(&core, cx, |c, cx| c.remove_server(cx, &server_id_for_distro(distro), failure_toast()));
    }
}
