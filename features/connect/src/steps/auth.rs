//! Sign-in steps: API key entry, OAuth code entry, browser OAuth wait (with optional redirect
//! URL paste), and server-side command execution output.

use rune::{El, TextStyle, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ButtonVariant, TextInput};
use rune_theme_opencode::icons as ic;
use session_view::palette::{Pal, kit};

use super::{current, input, provider_id, step_text};
use crate::machine::{self as conn, Event as Input, State as Conn};
use crate::ui::{self, BODY, LABEL};
use crate::{AuthHost, ConnectHost};

const MONO: TextStyle = TextStyle::new(12.0, 440.0).mono().lh(18.0);
/// The older field's value in code type, and its label (`text-field.css`).
const FIELD: TextStyle = TextStyle::new(14.0, 440.0).mono().lh(21.0);
const FIELD_LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(18.0);

/// The entry error under the API key or code field.
fn entry_error(c: &Conn, code: bool) -> Option<String> {
    match c {
        Conn::Entering(_, _, Some(conn::EntryError::Rejected(m))) => Some(m.clone()),
        Conn::Entering(_, _, Some(conn::EntryError::Required)) => Some(
            if code {
                opencode_i18n::t!("provider.connect.oauth.code.required")
            } else {
                opencode_i18n::t!("provider.connect.apiKey.required")
            }
            .into(),
        ),
        _ => None,
    }
}

fn submit_entry(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let value = host.connect().open.as_ref().map_or(String::new(), |o| o.entry.text.clone());
    input(host, Input::EntrySubmitted(value), tx);
}

fn submit_redirect(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let value = host.connect().open.as_ref().map_or(String::new(), |o| o.entry.text.clone());
    input(host, Input::PasteSubmit(value), tx);
}

fn toggle_paste(host: &mut dyn ConnectHost, locked: bool, shown: bool, tx: &mut Tx) {
    if locked {
        return;
    }
    input(host, Input::PasteToggle(!shown), tx);
    if let Some(o) = host.connect_mut().open.as_mut() {
        o.entry.focused = !shown;
    }
}

/// The code to check against the browser: the URL's `user_code`, or the device code after a colon
/// (`providers/connect/dialog.tsx:518-525`: prose instructions without `:` are not a code).
pub(super) fn confirmation_code(c: &Conn) -> Option<String> {
    let auth = conn::authorization(c)?;
    let user = auth
        .url
        .split(['?', '&'])
        .find_map(|kv| kv.strip_prefix("user_code="))
        .map(|v| v.replace("%2D", "-").replace('+', " "));
    user.or_else(|| {
        let i = auth.instructions.as_str();
        i.contains(':').then(|| i.rsplit(':').next().unwrap_or_default().trim().to_owned()).filter(|s| !s.is_empty())
    })
}

pub(super) fn wait_button(p: &Pal, title: &str) -> El {
    ui::button(title.to_owned(), ButtonVariant::Loading)
        .disabled(true)
        .build(kit(p))
        .key("connect-waiting")
        .role("status")
        .busy(true)
}

fn open_browser<H: ConnectHost + 'static>(p: &Pal) -> El {
    ui::button(opencode_i18n::t!("provider.connect.oauth.openBrowser"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut H, tx: &mut Tx| input(s, Input::BrowserOpen, tx))
        .build(kit(p))
        .key("connect-open-browser")
}

/// The older `TextField` read-only and copyable, in code type (`components/text-field.css` with
/// `font-mono`): an `input-base` box in a 1 px `border-base` border that takes its room, the
/// value 12 pt in, Copy at its end; its label above (unless hidden) and a description below.
pub(super) fn copy_field<H: ConnectHost + 'static>(p: &Pal, key: &str, label: Option<&str>, description: Option<&str>, value: String) -> El {
    let weak = p.v1_text_weak;
    let copy = value.clone();
    let copy_btn = el()
        .key("copy")
        .role("button")
        .label_static(opencode_i18n::t!("ui.message.copy"))
        .size(24.0, 24.0)
        .mt(4.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |_: &mut H, tx: &mut Tx| tx.copy(copy.clone()))
        .child(icon(&ic::COPY, 16.0).color(weak));
    build_copy_field_layout(p, key, label, description, value, copy_btn)
}

fn build_copy_field_layout(p: &Pal, key: &str, label: Option<&str>, description: Option<&str>, value: String, copy_btn: El) -> El {
    let weak = p.v1_text_weak;
    let fill = if p.theme == 0 {
        rune::color::hex(if p.dark { "#151515" } else { "#f8f8f8" }).unwrap_or(p.layer1)
    } else {
        p.layer1
    };
    let wrapper = row()
        .key(key.to_owned())
        .role("textbox")
        .label(label.unwrap_or_default().to_owned())
        .w_full()
        .pr(4.0)
        .radius(6.0)
        .fill(fill)
        .border(1.0, p.border)
        .border_layout()
        // A value wider than the box runs past its end, clipped, as an input's does.
        .child(
            row()
                .h(32.0)
                .px(12.0)
                .py(2.0)
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .center_y()
                .clip_soft(0.0)
                .child(text(value).font(FIELD).color(p.standard.text).selectable(true)),
        )
        .child(copy_btn);
    col()
        .w_full()
        .gap(8.0)
        .children(label.map(|l| text(l.to_owned()).font(FIELD_LABEL).color(weak)))
        .child(wrapper)
        .children(description.map(|d| text(d.to_owned()).font(BODY).color(p.muted).wrap()))
}

/// A labelled secret field, its error and Continue (`ApiAuthView`, `OAuthCodeView`).
fn entry_form<H: ConnectHost + 'static>(s: &H, p: &Pal, label: String, placeholder: &str, error: Option<String>, submitting: bool, console: bool) -> El {
    let Some(open) = s.connect().open.as_ref() else { return col() };
    let field = TextInput::new(&open.entry)
        .large()
        .secure()
        .spellcheck(false)
        .disabled(submitting)
        .placeholder(placeholder)
        .label(label.clone())
        .width(None)
        .invalid(error.is_some())
        .on_edit(|s: &mut H| s.connect_mut().entry_mut())
        .on_submit(|s: &mut H, tx: &mut Tx| submit_entry(s, tx));
    let field = if let Some(message) = &error { field.description(message) } else { field };
    let field = field.build(kit(p)).key("connect-entry");
    let mut cont = ui::button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast)
        .disabled(submitting)
        .on_press_tx(|s: &mut H, tx: &mut Tx| submit_entry(s, tx))
        .build(kit(p))
        .key("connect-submit")
        .busy(submitting);
    if console {
        cont = cont.px(12.0);
    }
    let browser_btn = matches!(conn::signin(current(s)), Some(conn::Signin::Code { .. })).then(|| open_browser::<H>(p));
    build_entry_form_layout(p, label, field, error, browser_btn, cont)
}

fn build_entry_form_layout(p: &Pal, label: String, field: El, error: Option<String>, browser_btn: Option<El>, cont: El) -> El {
    let mut form = col()
        .key("connect-entry-form")
        .w_full()
        .gap(20.0)
        .child(col().w_full().gap(8.0).child(text(label).font(LABEL).color(p.text)).child(field));
    if let Some(e) = error {
        form = form.child(text(e).key("connect-entry-error").role("alert").font(BODY).color(p.danger).mt(-16.0));
    }
    form.child(row().w_full().center_y().gap(8.0).child(spacer()).children(browser_btn).child(cont))
}

pub(super) fn key<H: ConnectHost + 'static>(s: &H, p: &Pal, console: bool) -> El {
    let c = current(s);
    let name = crate::provider_name(s, &provider_id(s));
    let about = if console {
        opencode_i18n::t!("provider.connect.console.serviceKeyDescription").to_owned()
    } else {
        opencode_i18n::t!("provider.connect.apiKey.description", provider = name)
    };
    col()
        .key("connect-key")
        .px(12.0)
        .gap(20.0)
        .pb(if console { 4.0 } else { 0.0 })
        .child(step_text(p, about))
        .child(entry_form(
            s,
            p,
            opencode_i18n::t!("provider.connect.apiKey.label", provider = name),
            opencode_i18n::t!("provider.connect.apiKey.placeholder"),
            entry_error(c, false),
            conn::submitting(c),
            console,
        ))
}

pub(super) fn code<H: ConnectHost + 'static>(s: &H, p: &Pal) -> El {
    let c = current(s);
    let name = crate::provider_name(s, &provider_id(s));
    let method = conn::method(c).map(|m| m.title()).unwrap_or_default();
    col()
        .key("connect-code")
        .px(12.0)
        .gap(20.0)
        .child(step_text(p, opencode_i18n::t!("provider.connect.oauth.code.description", provider = name)))
        .child(entry_form(
            s,
            p,
            opencode_i18n::t!("provider.connect.oauth.code.label", method = method),
            opencode_i18n::t!("provider.connect.oauth.code.placeholder"),
            entry_error(c, true),
            conn::submitting(c),
            false,
        ))
}

pub(super) fn browser<H: ConnectHost + 'static>(s: &H, p: &Pal) -> El {
    let Some(open) = s.connect().open.as_ref() else { return col() };
    let c = current(s);
    let name = crate::provider_name(s, &provider_id(s));
    let mut e = col()
        .key("connect-browser")
        .px(12.0)
        .gap(20.0)
        .child(step_text(p, opencode_i18n::t!("provider.connect.oauth.auto.description", provider = name)));
    if let Some(code) = confirmation_code(c) {
        e = e.child(copy_field::<H>(p, "connect-confirmation", Some(opencode_i18n::t!("provider.connect.oauth.auto.confirmationCode")), None, code));
    }
    let actions = row().w_full().gap(8.0).center_y().child(spacer()).child(open_browser::<H>(p));
    if let Some(conn::Signin::Browser { failed, paste, .. }) = conn::signin(c) {
        if *failed {
            e = e.child(text(opencode_i18n::t!("provider.connect.oauth.browserFailed")).role("alert").font(BODY).color(p.danger).wrap());
        }
        if *paste != conn::Paste::Unsupported && matches!(c, Conn::Waiting(..)) {
            let (shown, locked, err) = match paste {
                conn::Paste::Editing { open: expanded, error } => (expanded.unwrap_or(matches!(open.auth_host, AuthHost::Remote(_))), false, error.as_ref()),
                _ => (true, true, None),
            };
            let toggle = opencode_i18n::t!("provider.connect.oauth.paste.toggle");
            let mut block = col().w_full().child(
                row()
                    .key("connect-paste-toggle")
                    .role("button")
                    .label_static(toggle)
                    .expanded(shown)
                    .h(48.0)
                    .gap(6.0)
                    .center_y()
                    .on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_paste(s, locked, shown, tx))
                    .child(icon(if shown { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }, 14.0).mirror_in_rtl().color(p.icon_muted))
                    .child(text(toggle).font(BODY).color(p.text)),
            );
            if shown {
                let label = opencode_i18n::t!("ui.redirectPaste.label");
                let message = err.map(|e| match e {
                    conn::EntryError::Required => opencode_i18n::t!("ui.redirectPaste.required"),
                    conn::EntryError::Rejected(m) => m.as_str(),
                });
                let field = TextInput::new(&open.entry)
                    .large()
                    .width(None)
                    .label(label)
                    .placeholder(opencode_i18n::t!("ui.redirectPaste.placeholder"))
                    .spellcheck(false)
                    .disabled(locked)
                    .invalid(err.is_some())
                    .on_edit(|s: &mut H| s.connect_mut().entry_mut())
                    .on_change(|s: &mut H, tx: &mut Tx| input(s, Input::PasteEdited, tx))
                    .on_submit(|s: &mut H, tx: &mut Tx| submit_redirect(s, tx));
                let field = if let Some(msg) = message { field.description(msg) } else { field };
                let field = field.build(kit(p));
                block = block
                    .child(step_text(p, opencode_i18n::t!("provider.connect.oauth.paste.description").into()))
                    .child(text(label).font(LABEL).color(p.text))
                    .child(field.key("connect-redirect"));
                if let Some(msg) = message {
                    block = block.child(text(msg.to_owned()).role("alert").font(BODY).color(p.danger).wrap());
                }
            }
            e = e.child(block);
            if shown && !open.entry.text.trim().is_empty() && *paste != conn::Paste::Accepted {
                return e.child(
                    actions.child(
                        ui::button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast)
                            .disabled(locked)
                            .on_press_tx(|s: &mut H, tx: &mut Tx| submit_redirect(s, tx))
                            .build(kit(p))
                            .key("connect-paste-submit")
                            .busy(locked),
                    ),
                );
            }
        }
    }
    e.child(actions.child(wait_button(p, opencode_i18n::t!("provider.connect.status.waiting"))))
}

pub(super) fn command<H: ConnectHost + 'static>(s: &H, p: &Pal) -> El {
    let output = match current(s) {
        Conn::Waiting(_, conn::Signin::Command { output, .. }) => output.trim().to_owned(),
        _ => String::new(),
    };
    let mut e = col().key("connect-command").px(12.0).gap(16.0);
    if !output.is_empty() {
        // `max-h-60 overflow-auto`: a plain box's bar (`providers/connect/dialog.tsx:751`).
        let viewport = col().w_full().max_h(240.0).px(12.0).py(8.0).radius(6.0).fill(p.layer3);
        e = e.child(
            session_view::scroller::scroller("connect-command-output")
                .stick_bottom()
                .build::<H>(p, viewport, text(output).font(MONO).color(p.text).wrap().selectable(true)),
        );
    }
    e.child(row().w_full().child(spacer()).child(wait_button(p, opencode_i18n::t!("provider.connect.command.waiting"))))
}
