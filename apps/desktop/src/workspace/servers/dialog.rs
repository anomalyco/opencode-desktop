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

/// The dialog's open session: `state` and any redeemed pairing token live only while open, so
/// closing drops both and any late link-redeem completion is ignored.
#[derive(Default)]
enum Session {
    #[default]
    Closed,
    Open(Box<Open>),
}

struct Open {
    state: State,
    /// The code of the last link this form redeemed and what it got: the code is spent, so a retry
    /// after a failed check reuses its token (`useRedeemPairing`).
    redeemed: Option<(String, pairing::Pairing)>,
}

enum SessionEvent {
    Open(State),
    Step(Event),
    Redeemed(String, pairing::Pairing),
    Close,
}

impl Session {
    fn is_open(&self) -> bool {
        matches!(self, Self::Open(_))
    }

    fn state(&self) -> Option<&State> {
        match self {
            Self::Closed => None,
            Self::Open(open) => Some(&open.state),
        }
    }

    fn redeemed(&self) -> Option<&(String, pairing::Pairing)> {
        match self {
            Self::Closed => None,
            Self::Open(open) => open.redeemed.as_ref(),
        }
    }

    fn step(&mut self, event: SessionEvent) -> Option<Vec<Effect>> {
        match (self, event) {
            (session, SessionEvent::Open(state)) => {
                *session = Self::Open(Box::new(Open { state, redeemed: None }));
                None
            }
            (Self::Open(open), SessionEvent::Step(ev)) => Some(open.state.step(ev)),
            (Self::Open(open), SessionEvent::Redeemed(code, paired)) => {
                open.redeemed = Some((code, paired));
                None
            }
            (session, SessionEvent::Close) => {
                *session = Self::Closed;
                None
            }
            (Self::Closed, _) => None,
        }
    }
}

/// The open form and its fields.
#[derive(Default)]
pub struct Dialog {
    session: Session,
    pub link: TextEdit,
    pub url: TextEdit,
    pub name: TextEdit,
    pub password: TextEdit,
}

pub const OVERLAY_ID: &str = "dialog.servers";

pub(crate) static DIALOG: opencode_kernel::Slot<Dialog> = opencode_kernel::Slot::new();

/// Whether the Add/Edit server dialog is currently open.
pub fn is_open(s: &App) -> bool {
    s.kernel.slots.get(&DIALOG).session.is_open()
}

/// Opens Add server, or Edit server for saved server `url`.
pub fn open(s: &mut App, edit: Option<&str>, tx: &mut Tx) {
    tx.instant();
    let target = match edit.and_then(|url| super::list().into_iter().find(|x| x.url == url)) {
        Some(server) => Target::Edit(original(&server)),
        None => Target::Add,
    };
    let state = State::new(target);
    let d = s.kernel.slots.get_mut(&DIALOG);
    d.link = TextEdit::new(&state.draft.link);
    d.url = TextEdit::new(&state.draft.url);
    d.name = TextEdit::new(&state.draft.name);
    d.password = TextEdit::new(&state.draft.password);
    focus_primary(d, state.method);
    d.session.step(SessionEvent::Open(state));
    opencode_kernel::AppOverlays::on_overlay_opened(s, OVERLAY_ID, None);
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
    let Some(url) = s.kernel.slots.untracked(&DIALOG).session.state().and_then(|st| match &st.target {
        Target::Edit(current) => Some(current.url.clone()),
        Target::Add => None,
    }) else {
        return;
    };
    let now = super::list().into_iter().find(|x| x.url == url).map(|server| original(&server));
    dispatch(s, Event::OriginalSynced(now), tx);
}

fn dispatch(s: &mut App, event: Event, tx: &mut Tx) {
    let effects = {
        let d = s.kernel.slots.get_mut(&DIALOG);
        let Some(before) = d.session.state().map(|st| (st.method, st.draft.url.clone(), st.draft.link.clone())) else {
            return;
        };
        let Some(effects) = d.session.step(SessionEvent::Step(event)) else { return };
        // The machine moved an address from the link field to the address field.
        if let Some(state) = d.session.state() {
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
        }
        effects
    };
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
            let last = s.kernel.slots.untracked(&DIALOG).session.redeemed().cloned();
            let deliver = s.deliverer();
            crate::plugin_host::tasks::run(move || {
                let (event, redeemed) = pair(attempt, &link, preferred, last);
                deliver(Box::new(move |s: &mut App, tx: &mut Tx| {
                    if let Some((code, paired)) = redeemed {
                        s.kernel.slots.get_mut(&DIALOG).session.step(SessionEvent::Redeemed(code, paired));
                    }
                    dispatch(s, event, tx);
                }) as Apply);
            });
        }
        // A backstop for a check that never answers; the checks above have their own timeouts.
        Effect::TimerStart { attempt } => {
            let deliver = s.deliverer();
            rune::task::spawn_after("server-check-timeout", Duration::from_millis(machine::CONNECT_TIMEOUT_MS), move || {
                deliver(Box::new(move |s: &mut App, tx: &mut Tx| dispatch(s, Event::SubmitTimedOut { attempt }, tx)) as Apply);
            })
            .ok();
        }
        Effect::TimerStop => {}
        // Settings' per-server pages follow the server saved (`onServerChange` selects it).
        Effect::Save(Save::Add(mut server)) => {
            // Re-pairing an address already in the registry keeps its display name (#64139).
            if let Some(state) = s.kernel.slots.untracked(&DIALOG).session.state()
                && state.target == Target::Add && state.draft.name.trim().is_empty()
                && let Some(existing) = super::list().into_iter().find(|e| e.url == server.http.url).and_then(|e| e.name) {
                server.name = Some(existing);
            }
            follow(s, &server.http.url);
            super::add(s, server);
        }
        // The edit moved the server: the registry keeps it at its new address only.
        Effect::Save(Save::Replace { original, server }) => {
            follow(s, &server.http.url);
            super::remove(&original);
            super::add(s, server);
        }
        Effect::Close { .. } => {
            s.kernel.slots.get_mut(&DIALOG).session.step(SessionEvent::Close);
            opencode_kernel::AppOverlays::on_overlay_closed(s, OVERLAY_ID);
        }
    }
}

/// Settings' own server becomes the one saved at `url`.
fn follow(s: &mut App, url: &str) {
    if let Some(key) = pairing::normalize(url).and_then(|u| opencode_servers::ServerConn::http_server(&u)).map(|c| c.key()) {
        s.kernel.slots.get_mut(&crate::settings::SETTINGS_PAGES).server.key = key;
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

pub(crate) fn close(s: &mut App, tx: &mut Tx) {
    dispatch(s, Event::Disposed, tx);
    s.kernel.slots.get_mut(&DIALOG).session.step(SessionEvent::Close);
    opencode_kernel::AppOverlays::on_overlay_closed(s, OVERLAY_ID);
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
pub fn view(s: &App, p: &Pal, cx: &rune::Cx<App>) -> Vec<El> {
    let d = s.kernel.slots.get(&DIALOG);
    let Some(state) = d.session.state() else { return Vec::new() };
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
        let mut g = vec![label(t!("dialog.server.add.url")), field("dialog-server-url", &d.url, t!("dialog.server.add.placeholder"), invalid, false, |s| &mut s.kernel.slots.get_mut(&DIALOG).url, Field::Url)];
        g.extend(error);
        body = body.child(group(g));
    } else {
        let mut g = vec![label(t!("server.connect.link")), field("dialog-server-link", &d.link, t!("server.connect.link.placeholder"), invalid, false, |s| &mut s.kernel.slots.get_mut(&DIALOG).link, Field::Link)];
        g.extend(error);
        body = body.child(group(g));
    }
    body = body.child(group(vec![label(t!("dialog.server.add.name")), field("dialog-server-name", &d.name, t!("dialog.server.add.namePlaceholder"), false, false, |s| &mut s.kernel.slots.get_mut(&DIALOG).name, Field::Name)]));
    if state.method == Method::Password {
        body = body.child(group(vec![
            label(t!("dialog.server.add.password")),
            field("dialog-server-password", &d.password, t!("dialog.server.add.passwordPlaceholder"), false, true, |s| &mut s.kernel.slots.get_mut(&DIALOG).password, Field::Password),
            hint(t!("server.connect.password.hint")),
        ]));
    }
    // `ConnectMethodSwitch`: quiet, as a pairing link is the way in and the password the fallback.
    let (prompt, other, next) = match state.method {
        Method::Link => (t!("server.connect.password.prompt"), t!("server.connect.password.use"), Method::Password),
        Method::Password => (t!("server.connect.link.prompt"), t!("server.connect.link.use"), Method::Link),
    };
    let switch = crate::widgets::button(other, ButtonVariant::GhostMuted).disabled(v.disabled).on_press_tx(move |s: &mut App, tx: &mut Tx| dispatch(s, Event::MethodChanged(next), tx)).build(k).key("dialog-server-method");
    body = body.child(row().key("server-connect-method").w_full().min_h(28.0).gap(4.0).center_y().wrap().child(text(prompt).font(rune::TextStyle::new(13.0, 440.0).lh(20.0)).color(p.faint)).child(switch));
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
    // The clip goes on a child: on the card it would clip the card's own shadow too.
    let card = col().w_full().clip_rounded(12.0)
        .child(row().key("dialog-server-header").w_full().padding(16.0).center_y().child(text(title).font(rune::TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04)).color(p.text)))
        // `divider-v2`: a 1 point rule drawn at half its height (`scaleY(0.5)`), centred in it.
        .child(col().w_full().h(1.0).py(0.25).child(el().w_full().h(0.5).fill(p.q.border_strong)))
        .child(col().key("dialog-server-body").w_full().min_w(0.0).px(16.0).pt(16.0).pb(8.0).child(body))
        .child(row().key("dialog-server-footer").w_full().padding(16.0).gap(8.0).child(rune::spacer()).child(cancel).child(submit));
    // `<Dialog fit class="settings-server-dialog">` (`servers/connect/dialog.tsx`): the normal size.
    let card = col().radius(12.0).fill(p.bg_base).elevate(p.overlay).child(card);
    let card = crate::widgets::sized(card, crate::widgets::DialogSize::Normal, crate::widgets::Fit::Content, crate::widgets::dialog_window(cx, *s.zoom));
    let card = col().key("dialog-server").role("dialog").label(title).modal(true, |s: &mut App| {
        let mut tx = Tx::default();
        close(s, &mut tx);
    }).child(card);
    crate::widgets::modal("dialog-server-modal", p, card, None)
}
