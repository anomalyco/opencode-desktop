//! What a session or draft tab shows instead of itself while its server can't serve it
//! (`session/server-unavailable-panel.tsx`): `ServerUnavailablePanel` (unreachable, or an
//! incompatible version) and `ServerSignedOutPanel` (the server refused our credentials). A
//! plugin server's own cover (`PluginServerCover`) is the plugin host's.

use opencode_kit::servers::ServerKey;
use opencode_servers::health::{self, Health};
use rune::{El, Tx, col, icon, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;

/// `text-16-medium`: 16 on 28.8 at 530.
const TITLE: rune::TextStyle = rune::TextStyle::new(16.0, 530.0).lh(28.8);
/// The panel's body, 13 on 24 at 440 (`text-13-regular` inherits the page's line height).
const BODY: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(24.0);
/// The session id: IBM Plex Mono 13 at 530 on 16.
const ID: rune::TextStyle = rune::TextStyle::new(13.0, 530.0).mono().lh(16.0);

/// Why a route's server can't serve it (`ServerUnavailablePanel`'s `reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Unreachable,
    Incompatible,
}

/// What covers a routed session or draft before its own page (`route.tsx:93-135`,
/// `new-session/route.tsx:42-102`): the plugin's cover, then signed out, then an incompatible
/// server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    SignedOut,
    Incompatible,
}

/// The cover over a route on `server`, if any.
pub fn cover(server: &ServerKey) -> Option<Cover> {
    if crate::servers::runtime::signed_out(server) {
        return Some(Cover::SignedOut);
    }
    health::is_incompatible(crate::servers::runtime::health(server).as_ref()).then_some(Cover::Incompatible)
}

/// Whether `server`'s health says nothing answers it (`unreachable | stopped`), so a session it
/// hasn't given shows the unreachable panel.
pub fn unreachable(server: &ServerKey) -> bool {
    matches!(crate::servers::runtime::health(server), Some(Health::Stopped)) || health::is_failed(crate::servers::runtime::health(server).as_ref()) && !crate::servers::runtime::signed_out(server)
}

/// The panel for `cover` over a route on `server`; `close` names the tab its Close tab button
/// closes.
pub fn panel(p: &Pal, cover: Cover, server: &ServerKey, close: Option<&str>) -> El {
    match cover {
        Cover::SignedOut => signed_out(p, server),
        Cover::Incompatible => unavailable(p, Reason::Incompatible, server, close),
    }
}

/// `SessionStatePanel`'s frame: centred, 24 in from the sides, 168 clear below, 16 up
/// (`h-full px-6 pb-42 -mt-4 ... gap-4`).
fn frame(key: &str, children: Vec<El>) -> El {
    col().key(key.to_owned()).grow(1.0).w_full().min_h(0.0).px(24.0).pb(168.0 + 32.0).items_center().justify_center().gap(16.0).children(children)
}

fn heading(p: &Pal, title: String, description: String) -> El {
    col()
        .items_center()
        .gap(8.0)
        .child(text(title).font(TITLE).color(p.text).max_w(448.0).wrap().align(rune::Align::Center))
        .child(text(description).font(BODY).color(p.v1_text_weak).max_w(448.0).wrap().align(rune::Align::Center))
}

fn close_tab(p: &Pal, tab: &str) -> El {
    let tab = tab.to_owned();
    crate::widgets::button(opencode_i18n::t!("session.error.notFound.closeTab"), rune_kit::ButtonVariant::Neutral)
        .icon(&ic::XMARK_SMALL)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&tab, tx))
        .build(crate::palette::kit(p))
        .key("session-state-close")
}

/// `ServerUnavailablePanel`: the hexagonal warning in the danger ink, why, and Close tab.
pub fn unavailable(p: &Pal, reason: Reason, server: &ServerKey, close: Option<&str>) -> El {
    let name = crate::servers::runtime::label(server);
    let (key, title, description) = match reason {
        Reason::Unreachable => ("session-unreachable", opencode_i18n::t!("session.error.unreachable"), opencode_i18n::t!("session.error.unreachable.description", server = name).to_owned()),
        Reason::Incompatible => {
            let version = health::version(crate::servers::runtime::health(server).as_ref()).unwrap_or("1").to_owned();
            ("session-incompatible", opencode_i18n::t!("session.error.incompatible"), opencode_i18n::t!("session.error.incompatible.description", server = name, version = version).to_owned())
        }
    };
    let mut children = vec![icon(&ic::OUTLINE_HEXAGONAL_WARNING, 20.0).color(p.danger), heading(p, title.to_owned(), description)];
    children.extend(close.map(|tab| close_tab(p, tab)));
    frame(key, children)
}

/// `ServerSignedOutPanel`: the lock, signed out, and Sign in again for a server added by address
/// (its Edit server dialog takes a new password, as Home's Authenticate does).
pub fn signed_out(p: &Pal, server: &ServerKey) -> El {
    let name = crate::servers::runtime::label(server);
    let mut children = vec![
        icon(&ic::LOCK, 20.0).color(p.icon_muted),
        heading(p, opencode_i18n::t!("session.error.signedOut").to_owned(), opencode_i18n::t!("session.error.signedOut.description", server = name).to_owned()),
    ];
    if let Some(url) = saved_url(server) {
        children.push(
            crate::widgets::button(opencode_i18n::t!("session.error.signedOut.signIn"), rune_kit::ButtonVariant::Neutral)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| crate::servers::dialog::open(s, Some(&url), tx))
                .build(crate::palette::kit(p))
                .key("session-signed-out-sign-in"),
        );
    }
    frame("session-signed-out", children)
}

/// The session header's word for a signed-out server whose session stays on screen: what
/// [`signed_out`] says, in one line, with its Sign in again as a link.
pub fn signed_out_status(p: &Pal, server: &ServerKey) -> El {
    let label = text(opencode_i18n::t!("session.error.signedOut").to_owned()).font(crate::view::UI_SMALL).color(p.faint);
    let link = saved_url(server).map(|url| {
        text(opencode_i18n::t!("session.error.signedOut.signIn").to_owned())
            .key("connection-sign-in")
            .font(crate::view::UI_SMALL)
            .color(p.text)
            .cursor(rune::Cursor::Pointer)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| crate::servers::dialog::open(s, Some(&url), tx))
    });
    rune::row().key("connection-status").gap(6.0).center_y().child(label).children(link)
}

/// The `servers.json` address of server `key`, when it was added by address.
fn saved_url(key: &ServerKey) -> Option<String> {
    crate::servers::list().into_iter().find(|x| opencode_servers::ServerConn::http_server(&x.url).is_some_and(|c| c.key() == *key)).map(|x| x.url)
}

/// `SessionNotFoundPanel`: the server's name and the session's id, and Close tab.
pub fn not_found(p: &Pal, server: &ServerKey, id: &str, close: &str) -> El {
    let ident = col()
        .items_center()
        .gap(4.0)
        .child(text(crate::servers::runtime::label(server)).font(BODY).color(p.text))
        .child(text(id.to_owned()).font(ID).color(p.v1_text_base).fill(p.text.alpha(0.08)).radius(4.0).px(4.0).py(2.0));
    frame("session-not-found", vec![
        heading(p, opencode_i18n::t!("session.error.notFound").to_owned(), opencode_i18n::t!("session.error.notFound.description").to_owned()),
        ident,
        close_tab(p, close),
    ])
}

