//! The ChatGPT plan welcome (`DialogChatGPTPlanWelcome`): once, as the connect dialog closes
//! after OpenAI connected through ChatGPT token sharing, a small card says the plan is in use.

use opencode_i18n::t;
use rune::{Align, Cx, El, Key, KeyEvent, Link, LinkPaint, Spring, TextStyle, Transition, Tx, col, icon, text};
#[allow(deprecated)]
use rune_kit::{ButtonSize, ButtonVariant};
use rune_theme_opencode::provider_icons;

use crate::App;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY};

/// `text-[20px] font-[530] leading-7 tracking-[-0.3px]`.
const TITLE: TextStyle = TextStyle::new(20.0, 530.0).lh(28.0).tracking(-0.3);
const USAGE_URL: &str = "https://chatgpt.com/settings/usage";

impl App {
    /// Shows the welcome after a connect that earned it, unless it was ever shown.
    pub(super) fn welcome_chatgpt_plan(&mut self) {
        if self.settings.chatgpt_welcome_seen {
            return;
        }
        self.change_settings(|x| x.chatgpt_welcome_seen = true);
        self.connect.welcome = true;
    }
}

fn close(s: &mut App, tx: &mut Tx) {
    tx.instant();
    s.connect.welcome = false;
    s.composer.focused = s.has_composer() && !s.settings.open;
}

/// Esc and Enter close it, Enter as the focused Got it does.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !s.connect.welcome {
        return false;
    }
    if matches!(ev.key, Key::Escape | Key::Enter) {
        close(s, tx);
    }
    true
}

/// The card over the scrim: OpenAI's mark, the title and what it means, the usage link, Got it.
/// Got it is `button-v2` at size large: 32 tall, 15 in, which rune-kit's older `Large` keeps.
#[allow(deprecated)]
pub fn layer(s: &App, p: &Pal, cx: &Cx<App>) -> Vec<El> {
    if !s.connect.welcome {
        return vec![];
    }
    let zoom = s.zoom.max(0.5);
    let win_w = cx.layout_size("zoom").map_or(1280.0 / zoom, |[w, _]| w);
    let words = t!("provider.connect.chatgptWelcome.usage");
    let usage = text(words)
        .key("chatgpt-welcome-usage")
        .font(BODY)
        .color(p.muted)
        .link(Link::new(0..words.len(), USAGE_URL).role("link").label(words).hover(LinkPaint::new().color(p.text)))
        .mt(4.0);
    let confirm = widgets::button(t!("provider.connect.chatgptWelcome.confirm"), ButtonVariant::Contrast)
        .size(ButtonSize::Large)
        .on_press_tx(close)
        .build(kit(p))
        .key("chatgpt-welcome-confirm")
        .w_full()
        .mt(32.0);
    let card = widgets::overlay(p, col())
        .key("chatgpt-welcome")
        .role("dialog")
        .label_static(t!("provider.connect.chatgptWelcome.title"))
        .w((win_w - 32.0).min(390.0))
        .radius(12.0)
        .fill(p.layer1)
        .items_center()
        .px(32.0)
        .pt(36.0)
        .pb(32.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| close(s, &mut Tx::default()))
        .children(provider_icons::named("openai").map(|m| icon(m, 48.0).color(p.icon)))
        .child(text(t!("provider.connect.chatgptWelcome.title")).font(TITLE).color(p.text).wrap().align(Align::Center).max_w(270.0).mt(24.0))
        .child(text(t!("provider.connect.chatgptWelcome.description")).font(BODY).color(p.muted).wrap().align(Align::Center).mt(12.0))
        .child(usage)
        .child(confirm);
    widgets::modal("chatgpt-welcome", p, card, None)
}
