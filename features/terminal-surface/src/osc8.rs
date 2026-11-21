//! OSC 8 terminal hyperlink confirmation modal (`packages/gui-plugins/src/terminal/terminal.tsx:325`,
//! `hover-label.ts`). When an OSC 8 hyperlink's target URI differs from the visible cell text shown
//! in the terminal (or uses a non-web scheme), the user confirms the real target before anything
//! opens — preventing ANSI hyperlink spoofing.

use opencode_i18n::t;
use opencode_ui::palette::{Elevate as _, Pal, kit};
use opencode_ui::style::{CODE, UI};
use opencode_ui::widgets::{DialogSize, Fit, button, dialog_window, sized};
use rune::{Cx, Direction, El, TextStyle, Tx, col, el, row, text};
use rune_kit::ButtonVariant;

use crate::{FocusClaimEvent, TerminalSurfaceHost};

/// Kernel overlay ID for the terminal OSC 8 hyperlink confirmation modal.
pub const OSC8_OVERLAY_ID: &str = "terminal.osc8";

const TITLE: TextStyle = TextStyle::new(15.0, 500.0).lh(20.0);
const BODY: TextStyle = UI.lh(18.0);

/// An OSC 8 link click waiting on user confirmation.
#[derive(Clone, Debug)]
pub struct Pending {
    pub sid: String,
    pub visible: String,
    pub url: String,
    pub shift: bool,
}

/// Opens the confirmation modal for an OSC 8 terminal hyperlink.
pub fn ask(host: &mut dyn TerminalSurfaceHost, pending: Pending, tx: &mut Tx) {
    tx.instant();
    host.terminal_mut().pending_osc8 = Some(pending);
    host.on_overlay_opened(OSC8_OVERLAY_ID, None);
}

/// Handles Cancel (`false`) or Open (`true`) on the OSC 8 confirmation modal.
pub fn answer(host: &mut dyn TerminalSurfaceHost, open: bool, tx: &mut Tx) {
    tx.instant();
    let Some(p) = host.terminal_mut().pending_osc8.take() else { return };
    host.on_overlay_closed(OSC8_OVERLAY_ID);
    if open {
        if browser_policy::is_http_with_credentials(&p.url) {
            return;
        }
        let is_file = p.url.trim().get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:"));
        if is_file || browser_policy::is_raw_unc_href(&p.url) {
            host.open_link(&p.sid, &p.url, tx);
        } else {
            host.open_web_link(&p.sid, &p.url, p.shift, tx);
        }
    } else {
        drop(host.terminal_mut().focus_claim.step(FocusClaimEvent::Request));
    }
}

/// Renders the OSC 8 confirmation modal while open (`DialogSize::Normal`, `Fit::Content`).
pub fn dialog<H: TerminalSurfaceHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    let Some(pending) = &s.terminal().pending_osc8 else { return Vec::new() };
    let win = dialog_window(cx, s.zoom());
    let k = kit(p);
    let cancel = button(t!("common.cancel"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut H, tx: &mut Tx| answer(s, false, tx))
        .build(k)
        .key("terminal-osc8-cancel")
        .autofocus();
    let confirm = button(t!("rune.terminal.link.open"), ButtonVariant::Contrast)
        .on_press_tx(|s: &mut H, tx: &mut Tx| answer(s, true, tx))
        .build(k)
        .key("terminal-osc8-confirm");

    let display = browser_policy::format_osc8_target(&pending.url);
    let mut target_box = col()
        .key("terminal-osc8-target")
        .w_full()
        .padding(8.0)
        .gap(4.0)
        .radius(4.0)
        .fill(p.layer2)
        .clip_soft(0.0);
    let has_domain = !display.registrable.is_empty();
    if has_domain {
        let mut host_row = row().key("terminal-osc8-host").dir(Direction::Ltr).w_full().min_w(0.0);
        if !display.host_prefix.is_empty() {
            host_row = host_row.child(text(display.host_prefix).font(CODE).color(p.muted));
        }
        host_row = host_row.child(
            text(display.registrable)
                .key("terminal-osc8-domain")
                .font(TextStyle::new(CODE.size, 600.0).lh(18.0))
                .color(p.text),
        );
        if !display.host_suffix.is_empty() {
            host_row = host_row.child(text(display.host_suffix).font(CODE).color(p.muted));
        }
        target_box = target_box.child(host_row);
    }
    target_box = target_box.child(
        text(display.url_display)
            .key("terminal-osc8-url")
            .font(CODE)
            .color(if has_domain { p.muted } else { p.text })
            .dir(Direction::Ltr)
            .wrap(),
    );

    let card = col()
        .key("terminal-osc8-dialog")
        .role("dialog")
        .label_static(t!("rune.terminal.link.confirm"))
        .w_full()
        .padding(20.0)
        .gap(12.0)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .child(text(t!("rune.terminal.link.confirm")).font(TITLE).color(p.text))
        .child(text(t!("rune.terminal.link.description")).font(BODY).color(p.muted).wrap())
        .child(target_box)
        .child(row().w_full().justify_end().gap(8.0).pt(4.0).child(cancel).child(confirm))
        .modal_tx(true, |s: &mut H, tx: &mut Tx| answer(s, false, tx));
    let card = sized(card, DialogSize::Normal, Fit::Content, win);
    vec![
        el().key("terminal-osc8-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim),
        el().key("terminal-osc8-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}
