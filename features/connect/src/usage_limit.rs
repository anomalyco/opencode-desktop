//! ChatGPT usage limit dialog (`packages/app/src/providers/connect/chatgpt-usage-limit.tsx`,
//! `packages/app/src/session/usage-exceeded-dialogs.tsx`, `anomalyco/opencode#64926`).
//!
//! Shows when `session.step.failed` reports `provider.chatgpt-limit` (personal limit) or
//! `provider.chatgpt-workspace-limit` (ChatGPT Business workspace limit, newer servers only —
//! older servers emit `provider.chatgpt-limit` and fall back to the personal dialog). Throttled
//! to at most once per 24 hours via `chatgpt_usage_limit_seen_at`.

use opencode_i18n::t;
use opencode_ui::dialog::DialogSize;
use opencode_ui::palette::{Pal, kit};
use rune::{Align, Cx, El, Key, KeyEvent, Spring, TextStyle, Transition, Tx, col, icon, text};
#[allow(deprecated)]
use rune_kit::{ButtonSize, ButtonVariant};
use rune_theme_opencode::provider_icons;

use crate::ui::{self, BODY, Fit};
use crate::{ConnectHost, CopyStatus, Modal, ModalEvent};

/// `text-[20px] font-[530] leading-7 tracking-[-0.3px]`.
const TITLE: TextStyle = TextStyle::new(20.0, 530.0).lh(28.0).tracking(-0.3);
const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
/// `CHATGPT_USAGE_LIMIT_WINDOW` (`usage-exceeded-dialogs.tsx:23`): 24 hours.
pub const WINDOW_MS: i64 = 86_400_000;

/// Opens the ChatGPT usage limit dialog when not throttled (24 h) and no Connect modal is open.
/// Returns `true` when opened.
pub fn open(host: &mut dyn ConnectHost, workspace: bool, tx: &mut Tx) -> bool {
    if host.connect().is_modal_open() {
        return false;
    }
    let now = host.now_ms();
    if host.chatgpt_usage_limit_seen_at().is_some_and(|seen| now.saturating_sub(seen) < WINDOW_MS) {
        return false;
    }
    tx.instant();
    crate::apply_modal(host, ModalEvent::OpenUsageLimit { workspace, seen_at: now }, tx);
    true
}

/// Opens the ChatGPT usage limit dialog directly for scenes/previews (bypasses 24 h throttle).
pub fn preview(host: &mut dyn ConnectHost, workspace: bool, copy: CopyStatus) {
    host.connect_mut().modal = Modal::UsageLimit { workspace, copy };
}

/// Closes the ChatGPT usage limit dialog, notifies `OverlayStack`, and restores composer focus.
pub fn close(host: &mut dyn ConnectHost, tx: &mut Tx) {
    if !host.connect().is_usage_limit_open() {
        return;
    }
    tx.instant();
    crate::apply_modal(host, ModalEvent::CloseUsageLimit, tx);
}

fn primary_action(host: &mut dyn ConnectHost, workspace: bool, tx: &mut Tx) {
    if workspace {
        tx.copy(t!("provider.connect.chatgptUsageLimit.request").to_owned());
        tx.instant();
        crate::apply_modal(host, ModalEvent::UsageLimitCopy(CopyStatus::Copied), tx);
        return;
    }
    host.open_external(USAGE_URL, tx);
    close(host, tx);
}

/// Esc closes; Enter runs the autofocused primary action (Manage usage or Copy request for admin).
pub fn key(host: &mut dyn ConnectHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((workspace, _)) = host.connect().usage_limit() else {
        return false;
    };
    match ev.key {
        Key::Escape => close(host, tx),
        Key::Enter => primary_action(host, workspace, tx),
        _ => {}
    }
    true
}

/// The ChatGPT usage limit modal layer (`DialogSize::Normal`, content fit).
pub fn layer<H: ConnectHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    if !s.connect().is_usage_limit_open() {
        return Vec::new();
    }
    let win = ui::dialog_window(s, cx);
    dialog(s, p, win)
}

#[allow(deprecated)]
pub fn dialog<H: ConnectHost + 'static>(s: &H, p: &Pal, win: [f32; 2]) -> Vec<El> {
    let Some((workspace, copy)) = s.connect().usage_limit() else {
        return Vec::new();
    };
    let primary_label = if workspace {
        t!("provider.connect.chatgptUsageLimit.copyRequest")
    } else {
        t!("provider.connect.chatgptUsageLimit.manage")
    };
    let primary = ui::button(primary_label, ButtonVariant::Contrast)
        .size(ButtonSize::Large)
        .on_press_tx(move |s: &mut H, tx: &mut Tx| primary_action(s, workspace, tx))
        .build(kit(p))
        .key("chatgpt-usage-limit-primary")
        .w_full()
        .mt(32.0);
    let close_btn = ui::button(t!("provider.connect.chatgptUsageLimit.close"), ButtonVariant::Ghost)
        .size(ButtonSize::Large)
        .on_press_tx(|s: &mut H, tx: &mut Tx| close(s, tx))
        .build(kit(p))
        .key("chatgpt-usage-limit-close")
        .w_full()
        .mt(8.0);
    let card = build_card(p, workspace, copy, primary, close_btn)
        .modal(true, |s: &mut H| close(s, &mut Tx::default()));
    // Uses `DialogSize::Normal` preset (enforced by `scripts/dialog-size.sh`, matching `welcome.rs`).
    ui::sized_modal("chatgpt-usage-limit", p, DialogSize::Normal, Fit::Content, win, card)
}

fn build_card(p: &Pal, workspace: bool, copy: CopyStatus, primary: El, close_btn: El) -> El {
    let description = if workspace {
        t!("provider.connect.chatgptUsageLimit.workspaceDescription")
    } else {
        t!("provider.connect.chatgptUsageLimit.description")
    };
    let mut card = ui::overlay(p, col())
        .key("chatgpt-usage-limit")
        .role("dialog")
        .label_static(t!("provider.connect.chatgptUsageLimit.title"))
        .radius(12.0)
        .fill(p.layer1)
        .items_center()
        .px(32.0)
        .pt(36.0)
        .pb(32.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .children(provider_icons::named("openai").map(|m| icon(m, 48.0).color(p.icon)))
        .child(
            text(t!("provider.connect.chatgptUsageLimit.title"))
                .font(TITLE)
                .color(p.text)
                .wrap()
                .align(Align::Center)
                .max_w(270.0)
                .mt(24.0),
        )
        .child(
            text(description)
                .font(BODY)
                .color(p.muted)
                .wrap()
                .align(Align::Center)
                .mt(12.0),
        )
        .child(primary);
    match copy {
        CopyStatus::Idle => {}
        CopyStatus::Copied => {
            card = card.child(
                text(t!("provider.connect.chatgptUsageLimit.requestCopied"))
                    .key("chatgpt-usage-limit-copied")
                    .role("status")
                    .font(BODY)
                    .color(p.muted)
                    .wrap()
                    .align(Align::Center)
                    .mt(12.0),
            );
        }
        CopyStatus::Failed => {
            card = card
                .child(
                    text(t!("provider.connect.chatgptUsageLimit.requestCopyFailed"))
                        .key("chatgpt-usage-limit-copy-failed")
                        .role("status")
                        .font(BODY)
                        .color(p.muted)
                        .wrap()
                        .align(Align::Center)
                        .mt(12.0),
                )
                .child(
                    col()
                        .key("chatgpt-usage-limit-fallback")
                        .w_full()
                        .mt(8.0)
                        .padding(12.0)
                        .radius(6.0)
                        .fill(p.hover)
                        .child(
                            text(t!("provider.connect.chatgptUsageLimit.request"))
                                .font(BODY)
                                .color(p.text)
                                .wrap()
                                .selectable(true),
                        ),
                );
        }
    }
    card.child(close_btn)
}
