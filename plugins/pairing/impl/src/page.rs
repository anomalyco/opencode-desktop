//! Settings › Pairing and the Pair a device dialog (`page.tsx`), drawn with the app's own
//! settings rows, switches and buttons.

use opencode_plugin::abi::{BUTTON_GHOST, BUTTON_NEUTRAL, SHADOW_OVERLAY};
use opencode_plugin::prelude::*;

use crate::Pairing;
use crate::machine::{Dialog, Event, page_view};

pub const DIALOG: &str = "pair";

const CLOSE: &str = r#"<path d="M12.4446 3.55469L3.55566 12.4436M3.55566 3.55469L12.4446 12.4436" stroke="currentColor" stroke-linejoin="round" fill="none"/>"#;

/// The page (`SettingsLayout` › `SettingsSection`): Pair a device, then, while the service
/// answers, Remote access and Keep screen active; errors under the list.
pub fn page(s: &Pairing, cx: &mut ViewCx<Pairing>, _: &Props) -> El {
    s.want_state();
    let Some(state) = s.state.as_ref() else { return ui::settings_list("pairing-list", [pair_row(cx, true)]) };
    let v = page_view(state);
    let danger = cx.token("text-danger-base").unwrap_or(cx.pal().danger);
    let error = |words: &'static str| text(words).font(13.0, 440.0).line_height(20.0).color(danger).role("alert");
    let remote = ui::switch_control(v.remote_checked).key("remote-access-switch").label("Remote access").disabled(v.remote_disabled);
    let remote = if v.remote_disabled { remote } else { remote.on_press(cx.handler(move |s: &mut Pairing, cx| s.dispatch(cx, Event::Toggled(!v.remote_checked)))) };
    let screen = ui::switch_control(v.screen_checked).key("keep-screen-active-switch").label("Keep screen active").disabled(v.screen_disabled);
    let screen = if v.screen_disabled { screen } else { screen.on_press(cx.handler(move |s: &mut Pairing, cx| s.dispatch(cx, Event::ScreenToggled(!v.screen_checked)))) };
    let rows = [
        pair_row(cx, v.pair_disabled),
        ui::settings_row(
            "settings-remote-access",
            "Remote access",
            "Paired devices reach this computer's OpenCode server over the internet. Turning it off disconnects them until you pair them again.",
            remote,
        ),
        ui::settings_row("settings-keep-screen-active", "Keep screen active", "Prevent this computer\u{2019}s display from sleeping while OpenCode is running.", screen),
    ];
    let mut section = col().key("pairing-section").w_full().gap(16.0).child(ui::settings_list("pairing-list", rows));
    if v.remote_error {
        section = section.child(error("Could not update remote access. Try again."));
    }
    if v.screen_error {
        let retry = ui::button_with("Try again", BUTTON_GHOST).small().key("pairing-screen-retry").on_press(cx.handler(|s: &mut Pairing, cx| s.dispatch(cx, crate::machine::Event::ScreenRetry)));
        section = section.child(row().gap(8.0).center_y().child(error("Could not update the screen activity setting. Try again.")).child(retry));
    }
    if v.unavailable {
        section = section.child(error("Pairing is unavailable right now."));
    }
    section
}

fn pair_row(cx: &mut ViewCx<Pairing>, disabled: bool) -> El {
    let button = ui::button_with("Pair device", BUTTON_NEUTRAL).key("settings-pair-device").disabled(disabled).on_press(cx.handler(|s: &mut Pairing, cx| s.dispatch(cx, Event::DialogOpened)));
    ui::settings_row("pairing", "Pair a device", "Connect a phone, tablet, or another computer from anywhere through an end-to-end encrypted tunnel.", button)
}

/// The dialog (`Dialog fit`, at most 400 wide): its header, then the QR code with its countdown,
/// Refresh code and Copy link; or the setup's progress, or why it can't pair, in the code's frame.
pub fn dialog(s: &Pairing, cx: &mut ViewCx<Pairing>, _: &Props) -> El {
    let Some(state) = s.state.as_ref() else { return el() };
    let now = cx.now_ms();
    let p = cx.pal();
    let base = cx.token("text-text-base").unwrap_or(p.text);
    let muted = cx.token("text-text-muted").unwrap_or(p.muted);
    let danger = cx.token("text-danger-base").unwrap_or(p.danger);
    let close = row()
        .key("pairing-dialog-close")
        .role("button")
        .label("Close")
        .size(20.0, 20.0)
        .mt(-2.0)
        .mr(-2.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .instant_hover()
        .child(icon_id(intern_icon("pairing-close", CLOSE, 16.0), 16.0).color(p.icon_muted))
        .on_press(cx.handler(|s: &mut Pairing, cx| {
            cx.close_dialog();
            s.dispatch(cx, Event::DialogClosed);
        }));
    let header = row()
        .key("pairing-dialog-header")
        .w_full()
        .padding(16.0)
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(4.0)
                .child(text("Pair a device").font(15.0, 530.0).line_height(20.0).tracking(-0.13).color(base))
                .child(text("Scan the code with the OpenCode app on another device, or copy the link.").font(13.0, 440.0).line_height(20.0).tracking(-0.04).color(muted).wrap()),
        )
        .child(close);
    let frame = |fill: Rgba| col().key("pairing-qr-frame").w_full().h(368.0).radius(6.0).fill(fill);
    let layer1 = cx.token("background-bg-layer-01").unwrap_or(p.layer1);
    let message = |words: &'static str| text(words).font(13.0, 440.0).line_height(20.0).color(danger).role("alert");
    let mut body = col().key("pairing-dialog-body").w_full().gap(16.0).px(16.0).pb(16.0);
    let refresh = |cx: &mut ViewCx<Pairing>, disabled: bool| {
        ui::button_with("Refresh code", BUTTON_GHOST).key("pairing-refresh").disabled(disabled).on_press(cx.handler(|s: &mut Pairing, cx| {
            s.copied = None;
            s.dispatch(cx, Event::CodeRefresh);
        }))
    };
    body = match &state.dialog {
        Dialog::Ready { code, tunnel, url, refreshing, .. } => {
            let payload = serde_json::json!({ "code": code, "urls": [tunnel] }).to_string();
            let qr = crate::qr::icon(&payload);
            let bg = cx.token("background-bg-base").unwrap_or(p.bg_base);
            let mut square = frame(bg).role("img").label("Pairing QR code").padding(24.0).center();
            if let Some(id) = qr {
                square = square.child(icon_id(id, 320.0).color(base));
            }
            let left = state.remaining(now);
            let copied = s.copied.as_ref().is_some_and(|(u, _)| u == url);
            let link = url.clone();
            let copy = ui::button_with(if copied { "Copied" } else { "Copy link" }, BUTTON_NEUTRAL)
                .key("pairing-copy-link")
                .lead_icon(if copied { "check" } else { "copy" })
                .on_press(cx.handler(move |s: &mut Pairing, cx| s.copy_link(cx, &link)));
            let actions = row().gap(8.0).center_y().child(refresh(cx, refreshing.is_some())).child(copy);
            let countdown = text(format!("Expires in {}:{:02}", left / 60, left % 60)).font(12.0, 440.0).line_height(16.0).tabular().color(muted).role("timer");
            body.child(square).child(row().w_full().gap(8.0).center_y().child(countdown).child(spacer()).child(actions))
        }
        Dialog::Code { .. } | Dialog::TurningOn | Dialog::Tunnel { .. } => {
            let setup = !matches!(state.dialog, Dialog::Code { .. });
            let mut inner = frame(layer1).role("status").padding(24.0).gap(8.0).center().child(ui::loader(20.0));
            if setup {
                inner = inner
                    .child(text("Setting up remote access.").font(13.0, 530.0).line_height(16.0).color(base))
                    .child(text("This can take a minute.").font(12.0, 440.0).line_height(16.0).color(muted));
            }
            body.child(inner)
        }
        Dialog::Failed { .. } => {
            let retry = ui::button_with("Try again", BUTTON_NEUTRAL).key("pairing-retry").on_press(cx.handler(|s: &mut Pairing, cx| s.dispatch(cx, Event::Retry)));
            body.child(frame(layer1).padding(24.0).gap(12.0).center().child(message("Could not set up remote access.")).child(retry))
        }
        // A failed code fetch hides the link, so the way to try again stays next to the error.
        Dialog::CodeError { .. } => body.child(col().w_full().gap(4.0).items_center().child(message("Could not update pairing details. Try again.")).child(refresh(cx, false))),
        Dialog::Unavailable | Dialog::Closed => body.child(frame(layer1).padding(24.0).center().child(message("Pairing is unavailable right now."))),
    };
    col().key("pairing-dialog").w(400.0).fill(layer1).radius(6.0).shadow(SHADOW_OVERLAY).clip().child(header).child(body)
}
