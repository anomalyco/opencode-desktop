//! An SSH server's row in Settings › Servers (`row.tsx`): its health, its name and the SSH
//! badge, its version (or Authentication required), Authenticate while it waits for a sign-in,
//! and its menu (Connect or Authenticate, Delete).

use opencode_plugin::abi::BUTTON_GHOST_MUTED;
use opencode_plugin::prelude::*;

use crate::SshWindow;
use crate::controller::with;
use crate::machine::{Event, Stage};

pub fn server_row(s: &SshWindow, cx: &mut ViewCx<SshWindow>, props: &Props) -> El {
    let Some(core) = s.core() else { return cx.nothing() };
    let Some(item) = core.borrow().state.servers.iter().find(|r| r.config.id == props.id).cloned() else { return cx.nothing() };
    let health = opencode_plugin::system::server_health(&props.id);
    let version = (core.borrow().revision, health.clone(), cx.dark());
    cx.memo(&format!("row-{}", props.id), version, move |cx| {
        let p = cx.pal();
        let id = item.config.id.clone();
        let stage = item.stage();
        let pending = stage == Stage::Connecting;
        let dot = |c: Rgba| el().size(6.0, 6.0).my(3.5).radius(3.0).fill(c);
        let indicator = match stage {
            Stage::Ready if health.is_some() => dot(cx.token("icon-success-base").unwrap_or(p.success)),
            Stage::Ready | Stage::Connecting => row().w(6.0).h(14.0).center().child(ui::loader(12.0)),
            Stage::Authentication => row().w(6.0).h(14.0).center().child(icon("lock", 12.0).color(p.icon_muted)),
            Stage::Incompatible => icon("warning", 12.0).color(cx.token("icon-warning-base").unwrap_or(p.warning)),
            Stage::Failed => dot(cx.token("icon-critical-base").unwrap_or(p.danger)),
            Stage::Disconnected => dot(p.icon_faint),
        };
        let name = row()
            .gap(4.0)
            .center_y()
            .min_w(0.0)
            .child(text(crate::name(&item.config)).font(13.0, 530.0).line_height(16.0).color(p.text).truncate().shrink())
            .child(row().px(4.0).py(2.0).radius(3.0).border(1.0, p.border).child(text("SSH").font(9.0, 440.0).line_height(9.0).color(p.muted)));
        let meta = if stage == Stage::Authentication { "Authentication required".to_owned() } else { health.as_ref().map(|v| format!("v{v}")).unwrap_or_default() };
        let copy = col().grow(1.0).shrink().min_w(0.0).gap(6.0).child(name).child(text(meta).font(11.0, 440.0).line_height(11.0).color(p.muted));
        let lead = row().grow(1.0).shrink().min_w(0.0).gap(10.0).child(indicator).child(copy);
        let mut actions = row().gap(8.0).center_y();
        if stage == Stage::Authentication || pending {
            let label = if pending { "Connecting over SSH" } else { "Authenticate" };
            let target = id.clone();
            actions = actions.child(ui::button_with(label, BUTTON_GHOST_MUTED).small().loading(pending).disabled(pending).key(format!("ssh-row-auth-{id}")).on_press(cx.handler(move |s: &mut SshWindow, cx| s.connect(cx, &target))));
        }
        let anchor = format!("ssh-row-menu-{id}");
        let target = id.clone();
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
            .on_press(cx.handler(move |_: &mut SshWindow, cx| {
                let mut menu = MenuSpec::new("SSH server").group("SSH server");
                if stage == Stage::Authentication {
                    menu = menu.item_with("connect", "Authenticate", !pending, false);
                } else if stage != Stage::Ready {
                    menu = menu.item_with("connect", "Connect", !pending, false);
                }
                let menu = menu.separator().item("delete", "Delete").align_end();
                let target = target.clone();
                cx.open_menu::<SshWindow>(&anchor, &menu, move |s, cx, picked| match picked {
                    Some("connect") => s.connect(cx, &target),
                    Some("delete") => s.forget(cx, &target),
                    _ => cx.unchanged(),
                });
            }));
        row().key(format!("ssh-row-{id}")).w_full().gap(16.0).center_y().child(lead).child(actions.child(dots))
    })
}

impl SshWindow {
    /// Connect or Authenticate: a new attempt this window answers; its prompts open the dialog.
    pub fn connect(&mut self, cx: &mut ActionCx, id: &str) {
        let Some(core) = self.core() else { return cx.toast_error("Request failed", "SSH is unavailable") };
        let Some(config) = core.borrow().state.servers.iter().find(|r| r.config.id == id).map(|r| r.config.clone()) else { return };
        // Its prompts open a dialog of their own as they come (`dialog.next`).
        self.prompting = Some(config.id.clone());
        with(&core, cx, |c, cx| c.dispatch(cx, Event::Start { config, replace: false, background: false }));
        self.prompt_dialog(cx);
    }

    pub fn forget(&mut self, cx: &mut ActionCx, id: &str) {
        let Some(core) = self.core() else { return cx.toast_error("Request failed", "SSH is unavailable") };
        let id = id.to_owned();
        with(&core, cx, |c, cx| c.dispatch(cx, Event::Forget { id }));
    }
}
