//! Add server and Edit server (Desktop's `DialogServer`, `servers/connect/dialog.tsx` and
//! `controller.ts`): a pairing link, or an address and password, and an optional name. The form's
//! machine ([`super::machine`]) decides; this file draws it in Desktop's dialog frame and runs its
//! effects: redeeming the link, checking the server, saving it.
//!
//! Desktop's `.settings-server-dialog` frame rules never match (the class lands on the dialog's
//! content, not its root), so the dialog is the default `Dialog fit`: 480 wide, a 16-point header,
//! a divider, the body 16 in (8 at its foot) and a 16-point footer.

use std::time::Duration;

use rune::{El, TextEdit, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, TextInput};
use session_view::palette::Elevate;

use super::machine::{self, Effect, Error, Event, Field, Method, Original, Save, State, Target};
use super::pairing;
use crate::App;
use crate::jobs::Apply;
use crate::palette::{Pal, kit};
use opencode_i18n::t;

/// The open form and its fields.
#[derive(Default)]
pub struct Dialog {
    pub state: Option<State>,
    pub link: TextEdit,
    pub url: TextEdit,
    pub name: TextEdit,
    pub password: TextEdit,
    /// The code of the last link this form redeemed and what it got: the code is spent, so a retry
    /// after a failed check reuses its token (`useRedeemPairing`).
    redeemed: Option<(String, pairing::Pairing)>,
}

/// Opens Add server, or Edit server for saved server `url`.
pub fn open(s: &mut App, edit: Option<&str>, tx: &mut Tx) {
    tx.instant();
    let target = match edit.and_then(|url| super::list().into_iter().find(|x| x.url == url)) {
        Some(server) => Target::Edit(original(&server)),
        None => Target::Add,
    };
    let state = State::new(target);
    let d = &mut s.settings_pages.servers.dialog;
    d.link = TextEdit::new(&state.draft.link);
    d.url = TextEdit::new(&state.draft.url);
    d.name = TextEdit::new(&state.draft.name);
    d.password = TextEdit::new(&state.draft.password);
    d.redeemed = None;
    focus_primary(d, state.method);
    d.state = Some(state);
}

fn original(server: &super::Server) -> Original {
    Original { url: server.url.clone(), name: server.name.clone(), password: server.password.clone(), unauthorized: super::health(&server.url) == Some(super::Health::Unauthorized) }
}

fn focus_primary(d: &mut Dialog, method: Method) {
    d.link.focused = method == Method::Link;
    d.url.focused = method == Method::Password;
    d.name.focused = false;
    d.password.focused = false;
}

/// The list changed elsewhere: an edit follows its server, and closes once it's gone.
pub fn sync(s: &mut App, tx: &mut Tx) {
    let Some(Target::Edit(current)) = s.settings_pages.servers.dialog.state.as_ref().map(|st| &st.target) else { return };
    let now = super::list().into_iter().find(|x| x.url == current.url).map(|server| original(&server));
    dispatch(s, Event::OriginalSynced(now), tx);
}

fn dispatch(s: &mut App, event: Event, tx: &mut Tx) {
    let d = &mut s.settings_pages.servers.dialog;
    let Some(state) = d.state.as_mut() else { return };
    let before = (state.method, state.draft.url.clone(), state.draft.link.clone());
    let effects = state.step(event);
    // The machine moved an address from the link field to the address field.
    let (method, url, link) = (state.method, state.draft.url.clone(), state.draft.link.clone());
    if before != (method, url.clone(), link.clone()) {
        if d.url.text != url {
            d.url = TextEdit::new(&url);
        }
        if d.link.text != link {
            d.link = TextEdit::new(&link);
        }
        if before.0 != method {
            focus_primary(d, method);
        }
    }
    tx.instant();
    for effect in effects {
        run(s, effect);
    }
}

fn run(s: &mut App, effect: Effect) {
    match effect {
        Effect::ConnectHttp { attempt, http } => {
            let deliver = s.deliverer();
            crate::plugin_host::tasks::run(move || {
                let healthy = super::check(&http.url, http.password.as_deref(), Duration::from_secs(30)) == super::Health::Healthy;
                deliver(Box::new(move |s: &mut App, tx: &mut Tx| {
                    let event = if healthy { Event::SubmitSucceeded { attempt, http } } else { Event::SubmitFailed { attempt, error: Error::ConnectionFailed } };
                    dispatch(s, event, tx);
                }) as Apply);
            });
        }
        Effect::ConnectLink { attempt, link, preferred } => {
            let last = s.settings_pages.servers.dialog.redeemed.clone();
            let deliver = s.deliverer();
            crate::plugin_host::tasks::run(move || {
                let (event, redeemed) = pair(attempt, &link, preferred, last);
                deliver(Box::new(move |s: &mut App, tx: &mut Tx| {
                    if let Some(r) = redeemed {
                        s.settings_pages.servers.dialog.redeemed = Some(r);
                    }
                    dispatch(s, event, tx);
                }) as Apply);
            });
        }
        // A backstop for a check that never answers; the checks above have their own timeouts.
        Effect::TimerStart { attempt } => {
            let deliver = s.deliverer();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(machine::CONNECT_TIMEOUT_MS));
                deliver(Box::new(move |s: &mut App, tx: &mut Tx| dispatch(s, Event::SubmitTimedOut { attempt }, tx)) as Apply);
            });
        }
        Effect::TimerStop => {}
        // Settings' per-server pages follow the server saved (`onServerChange` selects it).
        Effect::Save(Save::Add(server)) => {
            follow(s, &server.http.url);
            super::add(s, server);
        }
        // The edit moved the server: the registry keeps it at its new address only.
        Effect::Save(Save::Replace { original, server }) => {
            follow(s, &server.http.url);
            super::remove(&original);
            super::add(s, server);
        }
        Effect::Close { .. } => s.settings_pages.servers.dialog.state = None,
    }
}

/// Settings' own server becomes the one saved at `url`.
fn follow(s: &mut App, url: &str) {
    if let Some(key) = pairing::normalize(url).and_then(|u| opencode_kit::servers::ServerConn::http_server(&u)).map(|c| c.key()) {
        s.settings_pages.server.key = key;
    }
}

/// `controller.ts`'s `pair`: redeems the link (or reuses the token of the code it redeemed last),
/// then checks the server, the edited address first.
fn pair(attempt: u64, value: &str, preferred: Option<String>, last: Option<(String, pairing::Pairing)>) -> (Event, Option<(String, pairing::Pairing)>) {
    let failed = |error| (Event::SubmitFailed { attempt, error }, None);
    let Some(link) = pairing::pairing_link(value) else {
        if pairing::legacy_pairing_link(value) {
            return failed(Error::Redeem(t!("server.connect.link.legacy").to_owned()));
        }
        return failed(Error::LinkInvalid);
    };
    let paired = match last.filter(|(code, _)| *code == link.code) {
        // The same code again: an address the user edited since wins.
        Some((_, p)) => pairing::Pairing { url: if link.urls.contains(&p.url) { p.url } else { link.urls.first().cloned().unwrap_or(p.url) }, password: p.password },
        None => match pairing::redeem(&link) {
            pairing::Redeemed::Paired(p) => p,
            pairing::Redeemed::Expired => return failed(Error::Redeem(t!("server.connect.link.expired").to_owned())),
            pairing::Redeemed::Unreachable => return failed(Error::Redeem(opencode_i18n::t!("server.connect.link.unreachable", url = link.urls.join(", ")).to_owned())),
        },
    };
    let redeemed = Some((link.code.clone(), paired.clone()));
    let healthy = |url: &str| super::check(url, Some(&paired.password), Duration::from_secs(30)) == super::Health::Healthy;
    if let Some(url) = preferred.filter(|u| *u != paired.url)
        && healthy(&url)
    {
        return (Event::SubmitSucceeded { attempt, http: machine::Http { url, password: Some(paired.password.clone()) } }, redeemed);
    }
    if healthy(&paired.url) {
        return (Event::SubmitSucceeded { attempt, http: machine::Http { url: paired.url.clone(), password: Some(paired.password.clone()) } }, redeemed);
    }
    (Event::SubmitFailed { attempt, error: Error::ConnectionFailed }, redeemed)
}

fn close(s: &mut App, tx: &mut Tx) {
    dispatch(s, Event::Disposed, tx);
    s.settings_pages.servers.dialog.state = None;
}

fn error_words(e: &Error) -> String {
    match e {
        Error::LinkInvalid => t!("server.connect.link.invalid").to_owned(),
        Error::LinkLegacy => t!("server.connect.link.legacy").to_owned(),
        Error::AddressInvalid => t!("server.connect.address.invalid").to_owned(),
        Error::Redeem(words) => words.clone(),
        Error::ConnectionFailed => t!("dialog.server.add.error").to_owned(),
    }
}

/// The dialog, while it's open.
#[allow(deprecated)]
pub fn view(s: &App, p: &Pal) -> Vec<El> {
    let d = &s.settings_pages.servers.dialog;
    let Some(state) = d.state.as_ref() else { return Vec::new() };
    let v = machine::view(state);
    let k = kit(p);
    let token = |name: &str, fallback| session_view::palette::token(p.dark, name).unwrap_or(fallback);
    let label = |words: &str| text(words.to_owned()).font(rune::TextStyle::new(13.0, 530.0).lh(16.0)).color(p.text);
    let hint = |words: &str| text(words.to_owned()).font(rune::TextStyle::new(13.0, 440.0).lh(20.0)).color(p.muted).wrap();
    let error = v.error.as_ref().map(|e| text(error_words(e)).font(rune::TextStyle::new(11.0, 440.0).lh(16.0)).color(token("state-fg-danger", p.danger)).wrap().role("alert").key("dialog-server-error"));
    let field = |key: &str, edit: &TextEdit, placeholder: &str, invalid: bool, secure: bool, pick: fn(&mut App) -> &mut TextEdit, which: Field| {
        let mut input = TextInput::new(edit).large().width(None).placeholder(placeholder.to_owned()).label(placeholder.to_owned()).invalid(invalid).disabled(v.disabled)
            .on_edit(move |s: &mut App| pick(s))
            .on_change(move |s: &mut App, tx: &mut Tx| {
                let value = pick(s).text.clone();
                dispatch(s, Event::FieldChanged(which, value), tx);
            })
            .on_submit(|s: &mut App, tx: &mut Tx| dispatch(s, Event::Submitted, tx));
        if secure {
            input = input.secure();
        }
        input.build(k).key(key.to_owned())
    };
    let group = |children: Vec<El>| col().w_full().min_w(0.0).gap(8.0).children(children);
    let mut body = col().key("dialog-server-form").w_full().min_w(0.0).gap(24.0);
    if v.signed_out {
        body = body.child(
            row().key("dialog-server-notice").role("status").w_full().gap(8.0).items_start().px(12.0).py(10.0).radius(6.0).border(1.0, p.border).border_layout().fill(p.layer1)
                .child(el().my(2.0).child(icon(crate::plugin_host::icons::resolve("", "lock"), 12.0).color(p.icon_muted)))
                .child(text(t!("dialog.server.signedOut")).font(rune::TextStyle::new(13.0, 440.0).lh(20.0)).color(p.text).wrap().grow(1.0).shrink().min_w(0.0)),
        );
    }
    let invalid = v.error.is_some();
    if state.method == Method::Password {
        let mut g = vec![label(t!("dialog.server.add.url")), field("dialog-server-url", &d.url, t!("dialog.server.add.placeholder"), invalid, false, |s| &mut s.settings_pages.servers.dialog.url, Field::Url)];
        g.extend(error);
        body = body.child(group(g));
    } else {
        let mut g = vec![label(t!("server.connect.link")), field("dialog-server-link", &d.link, t!("server.connect.link.placeholder"), invalid, false, |s| &mut s.settings_pages.servers.dialog.link, Field::Link)];
        g.extend(error);
        body = body.child(group(g));
    }
    body = body.child(group(vec![label(t!("dialog.server.add.name")), field("dialog-server-name", &d.name, t!("dialog.server.add.namePlaceholder"), false, false, |s| &mut s.settings_pages.servers.dialog.name, Field::Name)]));
    if state.method == Method::Password {
        body = body.child(group(vec![
            label(t!("dialog.server.add.password")),
            field("dialog-server-password", &d.password, t!("dialog.server.add.passwordPlaceholder"), false, true, |s| &mut s.settings_pages.servers.dialog.password, Field::Password),
            hint(t!("server.connect.password.hint")),
        ]));
    }
    // `ConnectMethodSwitch`: quiet, as a pairing link is the way in and the password the fallback.
    let (prompt, other, next) = match state.method {
        Method::Link => (t!("server.connect.password.prompt"), t!("server.connect.password.use"), Method::Password),
        Method::Password => (t!("server.connect.link.prompt"), t!("server.connect.link.use"), Method::Link),
    };
    let switch = crate::widgets::button(other, ButtonVariant::GhostMuted).disabled(v.disabled).on_press_tx(move |s: &mut App, tx: &mut Tx| dispatch(s, Event::MethodChanged(next), tx)).build(k).key("dialog-server-method");
    body = body.child(row().key("server-connect-method").w_full().min_h(28.0).gap(4.0).center().wrap().child(text(prompt).font(rune::TextStyle::new(13.0, 440.0).lh(20.0)).color(p.faint)).child(switch));
    let title = match state.target {
        Target::Add => t!("dialog.server.add.title"),
        Target::Edit(_) => t!("dialog.server.edit.title"),
    };
    let submit_label = if v.busy {
        t!("dialog.server.add.checking")
    } else if state.target == Target::Add {
        t!("dialog.server.add.button")
    } else {
        t!("common.save")
    };
    let cancel = crate::widgets::button(t!("common.cancel"), ButtonVariant::Neutral).disabled(v.disabled).on_press_tx(|s: &mut App, tx: &mut Tx| close(s, tx)).build(k).key("dialog-server-cancel");
    let submit = crate::widgets::button(submit_label, ButtonVariant::Contrast).disabled(v.disabled).on_press_tx(|s: &mut App, tx: &mut Tx| dispatch(s, Event::Submitted, tx)).build(k).key("dialog-server-submit");
    let card = col().w(480.0).radius(6.0).fill(p.layer1).elevate(p.overlay).clip_rounded(6.0)
        .child(row().key("dialog-server-header").w_full().padding(16.0).center_y().child(text(title).font(rune::TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04)).color(p.text)))
        .child(el().w_full().h(0.5).fill(p.q.border_strong))
        .child(col().key("dialog-server-body").w_full().min_w(0.0).px(16.0).pt(16.0).pb(8.0).child(body))
        .child(row().key("dialog-server-footer").w_full().padding(16.0).gap(8.0).child(rune::spacer()).child(cancel).child(submit));
    let card = col().key("dialog-server").role("dialog").label(title).modal(true, |s: &mut App| {
        let mut tx = Tx::default();
        close(s, &mut tx);
    }).child(card);
    crate::widgets::modal("dialog-server-modal", p, card, None)
}
