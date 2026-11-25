//! The ChatGPT plan welcome (`DialogChatGPTPlanWelcome`): once, as the connect dialog closes
//! after OpenAI connected through ChatGPT token sharing, a small card says the plan is in use.

use opencode_i18n::t;
use rune::{Align, Cx, El, Key, KeyEvent, Link, LinkPaint, Spring, TextStyle, Transition, Tx, col, icon, text};
#[allow(deprecated)]
use rune_kit::{ButtonSize, ButtonVariant};
use rune_theme_opencode::provider_icons;
use opencode_ui::dialog::DialogSize;
use opencode_ui::palette::{Pal, kit};

use crate::ui::{self, BODY, Fit};
use crate::{ConnectHost, ModalEvent};

/// `text-[20px] font-[530] leading-7 tracking-[-0.3px]`.
const TITLE: TextStyle = TextStyle::new(20.0, 530.0).lh(28.0).tracking(-0.3);
const USAGE_URL: &str = "https://chatgpt.com/settings/usage";

/// Shows the welcome after a connect that earned it, unless it was ever shown or another modal is open.
/// Returns `true` when the welcome dialog opened.
pub fn open(host: &mut dyn ConnectHost, tx: &mut Tx) -> bool {
    if host.chatgpt_welcome_seen() || host.connect().is_modal_open() {
        return false;
    }
    crate::apply_modal(host, ModalEvent::OpenWelcome, tx);
    true
}

/// Closes the ChatGPT plan welcome dialog, notifies `OverlayStack`, and restores composer focus.
pub fn close(host: &mut dyn ConnectHost, tx: &mut Tx) {
    if !host.connect().is_welcome_open() {
        return;
    }
    tx.instant();
    crate::apply_modal(host, ModalEvent::CloseWelcome, tx);
}

/// Esc and Enter close it, Enter as the focused Got it does.
pub fn key(host: &mut dyn ConnectHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !host.connect().is_welcome_open() {
        return false;
    }
    if matches!(ev.key, Key::Escape | Key::Enter) {
        close(host, tx);
    }
    true
}

/// The ChatGPT plan welcome modal layer (`DialogSize::Normal`, content fit).
pub fn layer<H: ConnectHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    if !s.connect().is_welcome_open() {
        return Vec::new();
    }
    let win = ui::dialog_window(s, cx);
    dialog(s, p, win)
}

/// The card over the scrim: OpenAI's mark, the title and what it means, the usage link, Got it.
/// Got it is `button-v2` at size large: 32 tall, 15 in, which rune-kit's older `Large` keeps.
#[allow(deprecated)]
pub fn dialog<H: ConnectHost + 'static>(s: &H, p: &Pal, win: [f32; 2]) -> Vec<El> {
    if !s.connect().is_welcome_open() {
        return Vec::new();
    }
    let confirm = ui::button(t!("provider.connect.chatgptWelcome.confirm"), ButtonVariant::Contrast)
        .size(ButtonSize::Large)
        .on_press_tx(|s: &mut H, tx: &mut Tx| close(s, tx))
        .build(kit(p))
        .key("chatgpt-welcome-confirm")
        .w_full()
        .mt(32.0);
    let card = build_card(p, confirm).modal(true, |s: &mut H| close(s, &mut Tx::default()));
    // `<Dialog fit>` (`providers/connect/chatgpt-welcome.tsx`).
    ui::sized_modal("chatgpt-welcome", p, DialogSize::Normal, Fit::Content, win, card)
}

fn build_card(p: &Pal, confirm: El) -> El {
    let words = t!("provider.connect.chatgptWelcome.usage");
    let usage = text(words)
        .key("chatgpt-welcome-usage")
        .font(BODY)
        .color(p.muted)
        .link(Link::new(0..words.len(), USAGE_URL).role("link").label(words).hover(LinkPaint::new().color(p.text)))
        // An `ExternalLink` with no cursor of its own: `a { cursor: default }` (`packages/ui/src/styles/base.css:84`).
        .cursor(rune::Cursor::Default)
        .mt(4.0);
    ui::overlay(p, col())
        .key("chatgpt-welcome")
        .role("dialog")
        .label_static(t!("provider.connect.chatgptWelcome.title"))
        .radius(12.0)
        .fill(p.layer1)
        .items_center()
        .px(32.0)
        .pt(36.0)
        .pb(32.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .children(provider_icons::named("openai").map(|m| icon(m, 48.0).color(p.icon)))
        .child(text(t!("provider.connect.chatgptWelcome.title")).font(TITLE).color(p.text).wrap().align(Align::Center).max_w(270.0).mt(24.0))
        .child(text(t!("provider.connect.chatgptWelcome.description")).font(BODY).color(p.muted).wrap().align(Align::Center).mt(12.0))
        .child(usage)
        .child(confirm)
}
