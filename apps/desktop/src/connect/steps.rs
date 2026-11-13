//! A provider's steps in the connect dialog (`ProviderSteps` in `providers/connect/dialog.tsx`):
//! the methods, a method's form, the API key and code entries, the browser and command waits,
//! failures, the accounts list, Claude Agent's setup, the Console's sign-in and the models to
//! start with.

use opencode_kit::integration::FieldKind;
use rune::{El, TextEdit, TextStyle, Tx, col, el, icon, row, spacer, text};
use rune_kit::{ButtonVariant, Checkbox, RadioItem, TextInput};
use rune_theme_opencode::icons as ic;

use super::dialog::{self, AccountAction, Body, CONSOLE, Dialog, Event, Frame, Step};
use super::form::{self, FieldError, Issue};
use super::machine::{self as conn, Event as Input, Screen, State as Conn};
use crate::App;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY, BODY_MED, LABEL, button};

const MONO: TextStyle = TextStyle::new(12.0, 440.0).mono().lh(18.0);
const CODE_DIGIT: TextStyle = TextStyle::new(20.0, 530.0).mono().lh(28.0);
/// The older field's value in code type, and its label (	ext-field.css).
const FIELD: TextStyle = TextStyle::new(14.0, 440.0).mono().lh(21.0);
const FIELD_LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(18.0);

/// Which step the inputs belong to: a new one starts them empty.
pub fn inputs_id(d: &Dialog) -> String {
    let Some(p) = &d.provider else { return "picker".into() };
    let step = match &p.step {
        Step::Models { .. } => "models".to_owned(),
        Step::Accounts { .. } => "accounts".to_owned(),
        s => match s.connect() {
            Some(Conn::Form(_, f)) => format!("form:{}", f.key()),
            Some(c) => format!("{:?}:{}", conn::screen(c), conn::sel(c).map_or(0, |s| s.method)),
            None => String::new(),
        },
    };
    format!("{}:{}:{step}", d.session, p.id)
}

/// A text step's prefilled answer.
pub fn field_default(d: &Dialog) -> String {
    match d.provider.as_ref().and_then(|p| p.step.connect()) {
        Some(Conn::Form(_, f)) => match &f.step.kind {
            form::Kind::Text { text, .. } => text.clone(),
            form::Kind::Choice { choice: form::Choice::Custom { text }, .. } => text.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// The providers a connection to `id` brings and their models, in the server's order.
pub fn connection_groups(s: &App, id: &str) -> Vec<(super::catalog::ProviderInfo, Vec<crate::pickers::Model>)> {
    let models = s.connection_models(id);
    let mut infos: Vec<super::catalog::ProviderInfo> = s.provider_infos(s.connect_dir()).unwrap_or_default().iter()
        .filter(|p| p.id == id || p.integration.as_deref() == Some(id)).cloned().collect();
    if infos.is_empty() && models.iter().any(|m| m.provider == id) {
        infos.push(super::catalog::ProviderInfo { id: id.to_owned(), name: s.provider_name(id), integration: None });
    }
    infos.into_iter().map(|p| {
        let list = models.iter().filter(|m| m.provider == p.id).cloned().collect();
        (p, list)
    }).filter(|(_, m): &(_, Vec<_>)| !m.is_empty()).collect()
}

fn input(s: &mut App, event: Input, tx: &mut Tx) {
    tx.instant();
    s.connect_event(Event::Input(event), tx);
}

fn open(s: &App) -> &super::Open {
    s.connect.open.as_ref().expect("open")
}

pub fn body(s: &App, p: &Pal, frame: Frame) -> El {
    let d = &open(s).dialog;
    let provider = d.provider.as_ref().expect("a provider step");
    let console = provider.id == CONSOLE;
    let content = match dialog::body(d) {
        Body::Models => return models(s, p, &provider.id),
        Body::Accounts => accounts(s, p),
        Body::Claude { checking, can_check, timed_out } => claude(s, p, checking, can_check, timed_out),
        Body::NoModels { pending, refreshing } => no_models(p, pending, refreshing),
        Body::ConsoleSignIn { copied } => col().px(12.0).child(console_sign_in(s, p, copied)),
        Body::Screen(Screen::Busy) => col().px(12.0).child(widgets::status_row(p, "connect-busy", opencode_i18n::t!("provider.connect.status.inProgress"))),
        Body::Screen(Screen::EnvironmentOnly) => col().px(12.0).child(text(opencode_i18n::t!("dialog.provider.environmentOnly")).font(BODY).color(p.muted)),
        Body::Screen(Screen::Methods) => methods(s, p),
        Body::Screen(Screen::Form) => form_view(s, p),
        Body::Screen(Screen::Failed) => col().px(12.0).child(failed(s, p, console)),
        Body::Screen(Screen::Command) => command(s, p),
        Body::Screen(Screen::Key) => key(s, p, console),
        Body::Screen(Screen::Code) => code(s, p),
        Body::Screen(Screen::Browser) => browser(s, p),
        Body::Picker => col(),
    };
    let notice = (console && !s.server_is_local()).then(|| col().px(12.0).mb(20.0).child(remote_notice(s, p)));
    // A provider step fits its content; the Console's steps scroll in theirs.
    let body = col().w_full().children(notice).child(content.w_full());
    // A plain `overflow-y-auto` box's bar (`providers/connect/dialog.tsx:1332`).
    let inner = if console {
        session_view::scroller::scroller("connect-content").build::<App>(p, col().w_full().min_h(0.0).pb(if frame == Frame::Accounts { 0.0 } else { 16.0 }), body)
    } else {
        col().key("connect-content").w_full().min_h(0.0).child(body)
    };
    let (px, pb) = if frame == Frame::Accounts { (6.0, 6.0) } else { (8.0, if console { 0.0 } else { 8.0 }) };
    col().key("connect-body").w_full().grow(1.0).shrink().min_h(0.0).px(px).pb(pb).child(inner)
}

fn methods(s: &App, p: &Pal) -> El {
    let c = open(s).dialog.provider.as_ref().and_then(|p| p.step.connect()).expect("connect");
    let list = conn::catalog(c).map(|c| c.methods.clone()).unwrap_or_default();
    col().key("connect-methods").children(list.into_iter().enumerate().map(|(at, m)| {
        row().key(format!("connect-method-{at}")).role("button").label(m.title()).w_full().h(36.0).px(12.0).gap(8.0).center_y().radius(6.0)
            .hover(p.hover).press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| input(s, Input::MethodSelected(at), tx))
            .child(text(m.title()).font(BODY_MED).color(p.text))
    }))
}

/// The entry error under the API key or code field.
fn entry_error(c: &Conn, code: bool) -> Option<String> {
    match c {
        Conn::Entering(_, _, Some(conn::EntryError::Rejected(m))) => Some(m.clone()),
        Conn::Entering(_, _, Some(conn::EntryError::Required)) => Some(if code { opencode_i18n::t!("provider.connect.oauth.code.required") } else { opencode_i18n::t!("provider.connect.apiKey.required") }.into()),
        _ => None,
    }
}

/// A labelled secret field, its error and Continue (`ApiAuthView`, `OAuthCodeView`).
fn entry_form(s: &App, p: &Pal, label: String, placeholder: &str, error: Option<String>, submitting: bool, console: bool) -> El {
    let field = TextInput::new(&open(s).entry)
        .placeholder(placeholder)
        .label(label.clone())
        .width(None)
        .invalid(error.is_some())
        .on_edit(|s: &mut App| &mut s.connect.open.as_mut().expect("open").entry)
        .on_submit(submit_entry)
        .build(kit(p))
        .key("connect-entry");
    let mut form = col().key("connect-entry-form").w_full().gap(20.0)
        .child(col().w_full().gap(8.0).child(text(label).font(LABEL).color(p.text)).child(field));
    if let Some(e) = error {
        form = form.child(text(e).key("connect-entry-error").role("alert").font(BODY).color(p.danger).mt(-16.0));
    }
    let mut cont = button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast).disabled(submitting).on_press_tx(submit_entry).build(kit(p)).key("connect-submit");
    if console {
        cont = cont.px(12.0);
    }
    form.child(row().child(cont))
}

fn submit_entry(s: &mut App, tx: &mut Tx) {
    let value = s.connect.open.as_ref().map_or(String::new(), |o| o.entry.text.clone());
    input(s, Input::EntrySubmitted(value), tx);
}

fn step_text(p: &Pal, words: String) -> El {
    text(words).font(BODY).color(p.muted).wrap().w_full()
}

fn key(s: &App, p: &Pal, console: bool) -> El {
    let c = current(s);
    let name = s.provider_name(&provider_id(s));
    let about = if console { opencode_i18n::t!("provider.connect.console.serviceKeyDescription").to_owned() } else {
        opencode_i18n::t!("provider.connect.apiKey.description", provider = name)
    };
    col().key("connect-key").px(12.0).gap(20.0).pb(if console { 4.0 } else { 0.0 })
        .child(step_text(p, about))
        .child(entry_form(s, p, opencode_i18n::t!("provider.connect.apiKey.label", provider = name), opencode_i18n::t!("provider.connect.apiKey.placeholder"), entry_error(c, false), conn::submitting(c), console))
}

fn open_browser(p: &Pal) -> El {
    button(opencode_i18n::t!("provider.connect.oauth.openBrowser"), ButtonVariant::Neutral).icon(&ic::ARROW_UP_RIGHT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| input(s, Input::BrowserOpen, tx)).build(kit(p)).key("connect-open-browser")
}

fn code(s: &App, p: &Pal) -> El {
    let c = current(s);
    let name = s.provider_name(&provider_id(s));
    let method = conn::method(c).map(|m| m.title()).unwrap_or_default();
    col().key("connect-code").px(12.0).gap(20.0)
        .child(step_text(p, opencode_i18n::t!("provider.connect.oauth.code.description", provider = name)))
        .child(open_browser(p))
        .child(entry_form(s, p, opencode_i18n::t!("provider.connect.oauth.code.label", method = method), opencode_i18n::t!("provider.connect.oauth.code.placeholder"), entry_error(c, true), conn::submitting(c), false))
}

/// The code to check against the browser: the URL's `user_code`, or the instructions' last part.
fn confirmation_code(c: &Conn) -> Option<String> {
    let auth = conn::authorization(c)?;
    let user = auth.url.split(['?', '&']).find_map(|kv| kv.strip_prefix("user_code=")).map(|v| v.replace("%2D", "-").replace('+', " "));
    user.or_else(|| {
        let i = auth.instructions.as_str();
        if i.is_empty() { None } else if i.contains(':') { i.rsplit(':').next().map(|t| t.trim().to_owned()) } else { Some(i.to_owned()) }
    })
}

/// The older `TextField` read-only and copyable, in code type (`components/text-field.css` with
/// `font-mono`): an `input-base` box in a 1 px `border-base` border that takes its room, the
/// value 12 pt in, Copy at its end; its label above (unless hidden) and a description below.
fn copy_field(p: &Pal, key: &str, label: Option<&str>, description: Option<&str>, value: String) -> El {
    let weak = p.v1_text_weak;
    let fill = if p.theme == 0 { rune::color::hex(if p.dark { "#151515" } else { "#f8f8f8" }).unwrap_or(p.layer1) } else { p.layer1 };
    let copy = value.clone();
    let wrapper = row().key(key.to_owned()).role("textbox").label(label.unwrap_or_default().to_owned()).w_full().pr(4.0).radius(6.0).fill(fill).border(1.0, p.border).border_layout()
        // A value wider than the box runs past its end, clipped, as an input's does.
        .child(row().h(32.0).px(12.0).py(2.0).grow(1.0).shrink().min_w(0.0).center_y().clip_soft(0.0)
            .child(text(value).font(FIELD).color(p.standard.text).selectable(true)))
        .child(el().key("copy").role("button").label_static(opencode_i18n::t!("ui.message.copy")).size(24.0, 24.0).mt(4.0).radius(6.0).center().hover(p.hover).press(p.pressed)
            .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.copy(copy.clone()))
            .child(icon(&ic::COPY, 16.0).color(weak)));
    col().w_full().gap(8.0)
        .children(label.map(|l| text(l.to_owned()).font(FIELD_LABEL).color(weak)))
        .child(wrapper)
        .children(description.map(|d| text(d.to_owned()).font(BODY).color(p.muted).wrap()))
}

fn browser(s: &App, p: &Pal) -> El {
    let c = current(s);
    let name = s.provider_name(&provider_id(s));
    let mut e = col().key("connect-browser").px(12.0).gap(20.0)
        .child(step_text(p, opencode_i18n::t!("provider.connect.oauth.auto.description", provider = name)))
        .child(widgets::status_row(p, "connect-waiting", opencode_i18n::t!("provider.connect.status.waiting")))
        .child(row().gap(8.0).child(open_browser(p)));
    if let Some(code) = confirmation_code(c) {
        e = e.child(copy_field(p, "connect-confirmation", Some(opencode_i18n::t!("provider.connect.oauth.auto.confirmationCode")), Some(opencode_i18n::t!("provider.connect.oauth.auto.confirmationCode.description")), code));
    }
    e
}

fn command(s: &App, p: &Pal) -> El {
    let output = match current(s) {
        Conn::Waiting(_, conn::Signin::Command { output, .. }) => output.trim().to_owned(),
        _ => String::new(),
    };
    let mut e = col().key("connect-command").px(12.0).gap(16.0);
    if !output.is_empty() {
        // `max-h-60 overflow-auto`: a plain box's bar (`providers/connect/dialog.tsx:751`).
        let viewport = col().w_full().max_h(240.0).px(12.0).py(8.0).radius(6.0).fill(p.layer3);
        e = e.child(session_view::scroller::scroller("connect-command-output").stick_bottom().build::<App>(p, viewport, text(output).font(MONO).color(p.text).wrap().selectable(true)));
    }
    e.child(widgets::status_row(p, "connect-waiting", opencode_i18n::t!("provider.connect.command.waiting")))
}

/// `ErrorRow`: the ban sign, why it failed, and Try again.
fn failed(s: &App, p: &Pal, console: bool) -> El {
    let error = match current(s) {
        Conn::Failed(_, e, _) => e.clone(),
        _ => String::new(),
    };
    let words = if console { error } else { opencode_i18n::t!("provider.connect.status.failed", error = error) };
    col().key("connect-failed").gap(if console { 16.0 } else { 12.0 })
        .child(row().w_full().gap(8.0)
            .child(icon(&ic::CIRCLE_BAN_SIGN, 14.0).color(p.danger).mt(3.0))
            .child(text(words).role("alert").font(BODY).color(p.text).wrap().grow(1.0).shrink()))
        .child(row().child(button(opencode_i18n::t!("provider.connect.console.retry"), ButtonVariant::Neutral).on_press_tx(|s: &mut App, tx: &mut Tx| input(s, Input::Retry, tx)).build(kit(p)).key("connect-retry")))
}

fn form_view(s: &App, p: &Pal) -> El {
    let Conn::Form(_, f) = current(s) else { return col() };
    let mut head = col().w_full().gap(4.0).child(text(f.title()).font(BODY_MED).color(p.text).wrap());
    if let Some(d) = &f.step.field.description {
        head = head.child(text(d.clone()).font(BODY).color(p.muted).wrap());
    }
    let controls = match &f.step.kind {
        form::Kind::External { url, browser } => external(p, f, url, browser),
        form::Kind::Multiselect { picked, custom } => multiselect(s, p, f, picked, custom),
        form::Kind::Choice { .. } => choice(s, p, f),
        form::Kind::Text { .. } => text_step(s, p, f),
    };
    let mut e = col().key(format!("connect-form-{}", f.key())).px(12.0).gap(16.0).child(head).child(controls);
    if let Some(err) = f.error() {
        let words = if err == FieldError::Required { opencode_i18n::t!("provider.connect.form.required") } else { opencode_i18n::t!("provider.connect.form.number") };
        e = e.child(text(words).key("connect-form-error").role("alert").font(BODY).color(p.danger));
    }
    e
}

fn form_input(s: &mut App, i: form::Input, tx: &mut Tx) {
    input(s, Input::Form(i), tx);
}

fn cont(p: &Pal, enabled: bool) -> El {
    button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast).disabled(!enabled)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let text = s.connect.open.as_ref().map(|o| o.field.text.clone()).unwrap_or_default();
            if let Some(Conn::Form(_, f)) = s.connect.open.as_ref().and_then(|o| o.dialog.provider.as_ref()).and_then(|p| p.step.connect()) {
                match f.step.kind {
                    form::Kind::Text { .. } => form_input(s, form::Input::TextChanged(text), tx),
                    form::Kind::Choice { choice: form::Choice::Custom { .. }, .. } => form_input(s, form::Input::CustomChanged(text), tx),
                    _ => {}
                }
            }
            form_input(s, form::Input::Submitted, tx);
        })
        .build(kit(p)).key("connect-continue")
}

fn external(p: &Pal, f: &form::Form, url: &str, browser: &form::Browser) -> El {
    let opening = matches!(browser, form::Browser::Opening { .. });
    let label = if f.can_finish() { opencode_i18n::t!("provider.connect.form.openAgain") } else { opencode_i18n::t!("provider.connect.form.open") };
    let mut e = col().w_full().gap(16.0).child(row().gap(8.0)
        .child(button(label, ButtonVariant::Neutral).icon(&ic::ARROW_UP_RIGHT).disabled(opening)
            .on_press_tx(|s: &mut App, tx: &mut Tx| form_input(s, form::Input::ExternalOpen, tx)).build(kit(p)).key("connect-open-link"))
        .child(button(opencode_i18n::t!("provider.connect.form.finished"), ButtonVariant::Contrast).disabled(!f.can_finish())
            .on_press_tx(|s: &mut App, tx: &mut Tx| form_input(s, form::Input::Submitted, tx)).build(kit(p)).key("connect-finished")));
    if *browser == form::Browser::Failed {
        e = e.child(text(opencode_i18n::t!("provider.connect.form.browserFailed")).role("alert").font(BODY).color(p.danger))
            .child(copy_field(p, "connect-link", None, None, url.to_owned()));
    }
    e
}

fn multiselect(s: &App, p: &Pal, f: &form::Form, picked: &[String], _custom: &str) -> El {
    let mut options = f.options();
    let extra: Vec<String> = picked.iter().filter(|v| !options.iter().any(|o| o.0 == **v)).cloned().collect();
    options.extend(extra.into_iter().map(|v| (v.clone(), v, None)));
    let mut boxes = col().gap(8.0);
    for (value, label, description) in options {
        let checked = picked.contains(&value);
        let mut c = Checkbox::new(checked).label(label);
        if let Some(d) = description {
            c = c.description(d);
        }
        boxes = boxes.child(c.on_toggle_tx(move |s: &mut App, tx: &mut Tx| form_input(s, form::Input::OptionToggled(value.clone(), !checked), tx)).build(kit(p)));
    }
    let mut e = col().w_full().gap(16.0).child(boxes);
    if f.step.field.custom {
        let field = TextInput::new(&open(s).field).placeholder(opencode_i18n::t!("rune.connect.form.placeholder")).label(opencode_i18n::t!("ui.messagePart.option.typeOwnAnswer")).width(None).spellcheck(s.settings.spellcheck)
            .on_edit(|s: &mut App| &mut s.connect.open.as_mut().expect("open").field)
            .on_submit(add_custom).build(kit(p)).grow(1.0).shrink();
        e = e.child(row().w_full().gap(8.0).center_y().child(field)
            .child(button(opencode_i18n::t!("provider.connect.form.add"), ButtonVariant::Neutral).on_press_tx(add_custom).build(kit(p))));
    }
    if let Some(issue) = f.issue() {
        let words = match issue {
            Issue::SelectOne => opencode_i18n::t!("provider.connect.form.selectOne").to_owned(),
            Issue::MinItems => opencode_i18n::t!("provider.connect.form.minItems", count = f.step.field.min_items.unwrap_or(0)),
            Issue::MaxItems => opencode_i18n::t!("provider.connect.form.maxItems", count = f.step.field.max_items.unwrap_or(0)),
        };
        e = e.child(text(words).font(BODY).color(p.muted));
    }
    e.child(row().child(cont(p, f.can_continue())))
}

fn add_custom(s: &mut App, tx: &mut Tx) {
    let text = s.connect.open.as_ref().map(|o| o.field.text.clone()).unwrap_or_default();
    form_input(s, form::Input::CustomChanged(text), tx);
    form_input(s, form::Input::CustomAdded, tx);
    if let Some(o) = s.connect.open.as_mut() {
        o.field = TextEdit::new("");
        o.field.focused = true;
    }
}

fn choice(s: &App, p: &Pal, f: &form::Form) -> El {
    let selected = f.selected();
    let mut radios = col().key("connect-radios").w_full().gap(12.0);
    for (value, label, description) in f.options() {
        let mut r = RadioItem::new(label, selected.as_deref() == Some(value.as_str()));
        if let Some(d) = description {
            r = r.description(d);
        }
        let key = format!("connect-radio-{value}");
        radios = radios.child(r.on_select_tx(move |s: &mut App, tx: &mut Tx| form_input(s, form::Input::ChoiceSelected(value.clone()), tx)).build(kit(p)).key(key));
    }
    if f.step.field.kind == FieldKind::String && f.step.field.custom {
        let custom = selected.as_deref() == Some("custom");
        radios = radios.child(RadioItem::new(opencode_i18n::t!("ui.messagePart.option.typeOwnAnswer"), custom)
            .on_select_tx(|s: &mut App, tx: &mut Tx| form_input(s, form::Input::ChoiceSelected("custom".into()), tx)).build(kit(p)));
        if custom {
            let placeholder = f.step.field.placeholder.clone().unwrap_or_else(|| opencode_i18n::t!("rune.connect.form.placeholder").into());
            radios = radios.child(col().pl(24.0).child(TextInput::new(&open(s).field).placeholder(placeholder).label(opencode_i18n::t!("ui.messagePart.option.typeOwnAnswer")).width(None).spellcheck(s.settings.spellcheck)
                .on_edit(|s: &mut App| &mut s.connect.open.as_mut().expect("open").field)
                .on_change(|s: &mut App, tx: &mut Tx| {
                    let text = s.connect.open.as_ref().map(|o| o.field.text.clone()).unwrap_or_default();
                    form_input(s, form::Input::CustomChanged(text), tx);
                })
                .build(kit(p))));
        }
    }
    let mut actions = row().gap(8.0).center_y().child(cont(p, f.can_continue()));
    if !f.step.field.required {
        actions = actions.child(button(opencode_i18n::t!("provider.connect.form.skip"), ButtonVariant::GhostMuted).on_press_tx(|s: &mut App, tx: &mut Tx| form_input(s, form::Input::Skipped, tx)).build(kit(p)));
    }
    col().w_full().gap(16.0).child(radios).child(actions)
}

fn text_step(s: &App, p: &Pal, f: &form::Form) -> El {
    let field = TextInput::new(&open(s).field)
        .placeholder(f.step.field.placeholder.clone().unwrap_or_default())
        .label(f.title())
        .width(None)
        .invalid(f.error().is_some())
        .on_edit(|s: &mut App| &mut s.connect.open.as_mut().expect("open").field)
        .on_change(|s: &mut App, tx: &mut Tx| {
            let text = s.connect.open.as_ref().map(|o| o.field.text.clone()).unwrap_or_default();
            form_input(s, form::Input::TextChanged(text), tx);
        })
        .on_submit(|s: &mut App, tx: &mut Tx| form_input(s, form::Input::Submitted, tx))
        .build(kit(p))
        .key("connect-field");
    col().w_full().gap(16.0).child(field).child(row().child(cont(p, true)))
}

/// Desktop's notice for a sign-in that a remote server stores.
fn remote_notice(s: &App, p: &Pal) -> El {
    let name = match &*s.source {
        crate::Source::Live { server, .. } => server.base.split("://").nth(1).unwrap_or(&server.base).to_owned(),
        crate::Source::Fixture => String::new(),
    };
    col().role("note").w_full().padding(12.0).radius(6.0).fill(p.layer2).border(1.0, p.border).border_layout()
        .child(text(opencode_i18n::t!("provider.connect.remote.title", server = name)).font(BODY_MED).color(p.text))
        .child(text(opencode_i18n::t!("provider.connect.remote.description")).font(BODY).color(p.muted).wrap())
}

/// The Console's device sign-in (`console.tsx`).
fn console_sign_in(s: &App, p: &Pal, copied: Option<bool>) -> El {
    let c = current(s);
    let failed = matches!(c, Conn::Waiting(_, conn::Signin::Browser { failed: true, .. }));
    let code = confirmation_code(c);
    let mut device = col().gap(8.0).child(text(opencode_i18n::t!("provider.connect.console.deviceCode")).font(BODY_MED).color(p.text));
    match &code {
        None => device = device.child(row().h(48.0).center_y().child(text(opencode_i18n::t!("provider.connect.console.openingBrowser")).font(BODY).color(p.muted))),
        Some(code) => {
            let mut digits = row().gap(4.0);
            for ch in code.chars() {
                digits = digits.child(if ch == '-' {
                    row().h(48.0).mx(4.0).center_y().child(text("-").font(CODE_DIGIT).color(p.muted))
                } else {
                    row().h(48.0).w(32.0).center().radius(6.0).border(1.0, p.border).fill(p.layer2).child(text(ch.to_string()).font(CODE_DIGIT).color(p.text))
                });
            }
            device = device.child(digits.label(opencode_i18n::t!("provider.connect.console.deviceCode.label", code = code)))
                .child(text(opencode_i18n::t!("provider.connect.console.waiting")).role("status").font(BODY).color(p.muted));
        }
    }
    let label = if copied == Some(true) { opencode_i18n::t!("provider.connect.console.linkCopied") } else { opencode_i18n::t!("provider.connect.console.copyLink") };
    let mut fallback = row().key("console-browser-fallback").min_h(28.0).gap(12.0).center_y()
        .child(text(opencode_i18n::t!("provider.connect.console.browserHint")).font(BODY).color(p.faint))
        .child(button(label, ButtonVariant::GhostMuted).disabled(code.is_none()).on_press_tx(copy_link).build(kit(p)));
    if failed || copied == Some(false) {
        fallback = fallback.child(button(opencode_i18n::t!("provider.connect.console.openAgain"), ButtonVariant::Ghost).on_press_tx(|s: &mut App, tx: &mut Tx| input(s, Input::BrowserOpen, tx)).build(kit(p)));
    }
    let lead = if failed { opencode_i18n::t!("provider.connect.console.browserFailed") } else { opencode_i18n::t!("provider.connect.console.instructions") };
    let mut e = col().key("console-authorization").gap(20.0).child(step_text(p, lead.into())).child(device).child(fallback);
    if copied == Some(false) {
        e = e.child(text(opencode_i18n::t!("provider.connect.console.copyFailed")).role("alert").font(BODY).color(p.muted));
    }
    e
}

fn copy_link(s: &mut App, tx: &mut Tx) {
    let Some(o) = s.connect.open.as_ref() else { return };
    let Some(auth) = o.dialog.provider.as_ref().and_then(|p| p.step.connect()).and_then(conn::authorization).cloned() else { return };
    tx.copy(auth.url.clone());
    let session = o.dialog.session;
    s.connect_event(Event::LinkCopied { session, attempt_id: auth.attempt, copied: true }, tx);
}

fn no_models(p: &Pal, pending: bool, refreshing: bool) -> El {
    let about = if pending { opencode_i18n::t!("provider.connect.console.modelsLoading") } else {
        opencode_i18n::t!("provider.connect.console.noModels")
    };
    let mut actions = row().gap(8.0);
    if !pending {
        actions = actions.child(button(opencode_i18n::t!("provider.connect.console.openAgain"), ButtonVariant::Neutral).on_press_tx(|_: &mut App, tx: &mut Tx| {
            let _ = tx.open_url("https://opencode.ai/console");
        }).build(kit(p)));
    }
    actions = actions.child(button(opencode_i18n::t!("provider.connect.console.refresh"), ButtonVariant::Neutral).disabled(refreshing)
        .on_press_tx(|s: &mut App, tx: &mut Tx| input(s, Input::Refresh, tx)).build(kit(p)));
    col().key("connect-no-models").role("status").px(12.0).gap(20.0)
        .child(col().child(row().gap(8.0).center_y().child(icon(&ic::CIRCLE_CHECK, 16.0).color(p.text)).child(text(opencode_i18n::t!("provider.connect.console.connected")).font(BODY_MED).color(p.text)))
            .child(step_text(p, about.into())))
        .child(actions)
}

fn claude(s: &App, p: &Pal, checking: bool, can_check: bool, timed_out: bool) -> El {
    let problem = s.integration_of(s.connect_dir(), dialog::CLAUDE).and_then(|i| i.accounts().first().and_then(|a| a.status.clone()));
    let words = problem.as_ref().map(|st| st.message.clone())
        .or_else(|| timed_out.then(|| opencode_i18n::t!("provider.connect.claudeAgent.timeout").to_owned()));
    let mut e = col().key("claude-agent-setup").px(12.0).pb(8.0).gap(20.0)
        .child(step_text(p, opencode_i18n::t!("provider.connect.claudeAgent.description").into()));
    if let Some(w) = words {
        e = e.child(row().gap(8.0).child(icon(&ic::CIRCLE_BAN_SIGN, 14.0).color(p.danger).mt(3.0)).child(text(w).role("alert").font(BODY).color(p.text).wrap().grow(1.0).shrink()));
    }
    e = e.child(copy_field(p, "claude-command", None, None, "claude auth login".into()));
    if checking {
        return e.child(widgets::status_row(p, "claude-checking", opencode_i18n::t!("provider.connect.claudeAgent.checking")));
    }
    let again = s.accounts_of(dialog::CLAUDE).count > 0;
    let url = problem.and_then(|st| st.url).unwrap_or_else(|| "https://code.claude.com/docs/en/setup".into());
    e.child(row().gap(8.0)
        .child(button(if again { opencode_i18n::t!("provider.connect.claudeAgent.checkAgain") } else { opencode_i18n::t!("provider.connect.claudeAgent.connect") }, ButtonVariant::Contrast).disabled(!can_check)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                let replacing = s.integration_of(s.connect_dir(), dialog::CLAUDE).map(|i| i.accounts().iter().map(|a| a.id.clone()).collect()).unwrap_or_default();
                s.connect_event(Event::ClaudeCheck { replacing }, tx);
            }).build(kit(p)).key("connect-submit"))
        .child(button(opencode_i18n::t!("provider.connect.claudeAgent.install"), ButtonVariant::Neutral).icon(&ic::ARROW_UP_RIGHT)
            .on_press_tx(move |_: &mut App, tx: &mut Tx| {
                let _ = tx.open_url(&url);
            }).build(kit(p))))
}

/// A connected integration's accounts (`AccountsView`): pick the active one, rename or remove
/// one from its options, or add another.
fn accounts(s: &App, p: &Pal) -> El {
    let o = open(s);
    let Some(Step::Accounts { action, .. }) = o.dialog.provider.as_ref().map(|p| &p.step) else { return col() };
    let id = provider_id(s);
    let busy = matches!(action, AccountAction::Activating { .. } | AccountAction::Removing { .. });
    let active = match action {
        AccountAction::Activating { account, .. } => Some(account.clone()),
        _ => o.dialog.provider.as_ref().and_then(|p| p.accounts.active.clone()),
    };
    let mut list: Vec<_> = s.integration_of(s.connect_dir(), &id).map(|i| i.accounts().into_iter().cloned().collect()).unwrap_or_default();
    list.sort_by(|a: &opencode_kit::integration::Connection, b| rune::text::compare_names(&a.label, &b.label).then(a.id.cmp(&b.id)));
    let mut rows = col().key("provider-accounts").role("radiogroup").label_static(opencode_i18n::t!("dialog.provider.accounts.connected")).gap(2.0);
    for a in list {
        if *action == AccountAction::Confirming(a.id.clone()) {
            let confirm = a.id.clone();
            rows = rows.child(row().min_h(28.0).pl(28.0).pr(2.0).gap(6.0).center_y().radius(6.0)
                .child(text(opencode_i18n::t!("dialog.provider.accounts.removeConfirm", account = a.label)).role("alert").font(BODY).color(p.text).grow(1.0).shrink().truncate())
                .child(button(opencode_i18n::t!("common.cancel"), ButtonVariant::GhostMuted).on_press_tx(|s: &mut App, tx: &mut Tx| s.connect_event(Event::RemoveCancelled, tx)).build(kit(p)))
                .child(button(opencode_i18n::t!("dialog.provider.accounts.remove"), ButtonVariant::Neutral).on_press_tx(move |s: &mut App, tx: &mut Tx| s.connect_event(Event::RemoveConfirmed(confirm.clone()), tx)).build(kit(p))));
            continue;
        }
        if *action == AccountAction::Renaming(a.id.clone()) {
            let credential = a.id.clone();
            // `account-name.tsx`'s InlineInput is `dir="auto"`.
            let field = TextInput::new(&o.rename).label(opencode_i18n::t!("settings.providers.account.name")).width(None).plain().spellcheck(s.settings.spellcheck).dir(rune::Direction::Auto)
                .on_edit(|s: &mut App| &mut s.connect.open.as_mut().expect("open").rename)
                .on_submit(move |s: &mut App, tx: &mut Tx| rename_account(s, &credential, tx))
                .on_key(|s: &mut App, ev, tx| {
                    if ev.key != rune::Key::Escape {
                        return false;
                    }
                    s.connect_event(Event::RenameClosed, tx);
                    true
                })
                .build(kit(p));
            rows = rows.child(row().key(format!("account-rename-{}", a.id)).w_full().min_h(28.0).pl(24.0).pr(8.0).center_y().radius(6.0).fill(p.hover).child(field));
            continue;
        }
        let is_active = active.as_deref() == Some(a.id.as_str());
        let pick = a.clone();
        let mut line = row().key(format!("account-{}", a.id)).role("radio").label(a.label.clone()).grow(1.0).shrink().min_h(28.0).pl(6.0).pr(8.0).gap(6.0).center_y()
            .child(el().size(16.0, 16.0).center().children(is_active.then(|| icon(&ic::CHECK_SMALL, 14.0).color(p.icon))))
            .child(text(a.label.clone()).font(BODY).color(p.text).shrink().truncate());
        if a.status.is_some() {
            line = line.child(spacer()).child(text(opencode_i18n::t!("dialog.provider.needsSignIn")).font(BODY).color(p.warning));
        }
        if !busy {
            line = line.on_press_tx(move |s: &mut App, tx: &mut Tx| choose_account(s, &pick, tx));
        }
        let menu_id = a.id.clone();
        let options = el().key(format!("account-options-{}", a.id)).role("button").label(opencode_i18n::t!("dialog.provider.accounts.options", account = a.label)).size(24.0, 24.0).mr(4.0).radius(4.0).center()
            .hover(p.hover).press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                if let Some(o) = s.connect.open.as_mut() {
                    o.menu = if o.menu.as_deref() == Some(menu_id.as_str()) { None } else { Some(menu_id.clone()) };
                }
            })
            .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
        let mut account_row = row().w_full().min_h(28.0).radius(6.0).center_y().hover(p.hover).child(line).child(options);
        if o.menu.as_deref() == Some(a.id.as_str()) {
            account_row = account_row.child(account_menu(p, &a.id));
        }
        rows = rows.child(account_row);
    }
    let can_add = !busy && o.dialog.provider.as_ref().and_then(|p| p.step.connect()).and_then(conn::catalog).is_some_and(|c| !c.methods.is_empty());
    let mut add = row().key("provider-account-add").role("button").label_static(opencode_i18n::t!("dialog.provider.accounts.add")).w_full().h(28.0).pl(6.0).pr(8.0).gap(6.0).center_y().radius(6.0)
        .hover(p.hover).press(p.pressed)
        .child(el().size(16.0, 16.0).center().child(icon(&ic::PLUS, 16.0).color(p.icon_muted)))
        .child(text(opencode_i18n::t!("dialog.provider.accounts.add")).font(BODY).color(p.muted));
    add = if can_add { add.on_press_tx(|s: &mut App, tx: &mut Tx| s.connect_event(Event::AccountAdd, tx)) } else { add.opacity(0.5) };
    col().key("connect-accounts").w_full().gap(2.0)
        .child(el().h(0.5).mt(4.0).mb(6.0).mx(-6.0).fill(p.border_muted))
        .child(rows)
        .child(add)
}

/// Saves the account's new name (`AccountNameInput`), then leaves the field.
fn rename_account(s: &mut App, credential: &str, tx: &mut Tx) {
    let label = s.connect.open.as_ref().map(|o| o.rename.text.trim().to_owned()).unwrap_or_default();
    s.connect_event(Event::RenameClosed, tx);
    if label.is_empty() {
        return;
    }
    if !s.can_write() {
        return crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), crate::jobs::read_only());
    }
    let (credential, dir) = (credential.to_owned(), s.connect_dir().map(str::to_owned));
    s.job(move |srv| {
        let r = srv.credential_rename(&credential, &label);
        Box::new(move |s: &mut App, _: &mut Tx| {
            if let Err(e) = r {
                crate::status_toast::fail(s, opencode_i18n::t!("common.requestFailed"), e);
            }
            s.sync_catalogs(dir, |_, _, _| {});
        }) as crate::jobs::Apply
    });
}

fn account_menu(p: &Pal, id: &str) -> El {
    let (rename, remove) = (id.to_owned(), id.to_owned());
    let item = |key: &str, label: &str| row().key(key.to_owned()).role("menuitem").label(label.to_owned()).w_full().h(28.0).px(8.0).center_y().radius(4.0)
        .hover(p.hover).child(text(label.to_owned()).font(BODY).color(p.text));
    widgets::overlay(p, col()).key("account-menu").pin(None, Some(28.0), Some(4.0), None).w(160.0).padding(4.0).radius(8.0).fill(p.bg_base).raise()
        .child(item("account-rename", opencode_i18n::t!("common.rename")).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let label = s.integration_of(s.connect_dir(), &provider_id(s))
                .and_then(|i| i.connections.iter().find(|c| c.id == rename).map(|c| c.label.clone())).unwrap_or_default();
            s.connect_event(Event::RenameStarted(rename.clone()), tx);
            if let Some(o) = s.connect.open.as_mut() {
                o.menu = None;
                o.rename = TextEdit::new(&label);
                o.rename.focused = true;
            }
        }))
        .child(item("account-remove", opencode_i18n::t!("dialog.provider.accounts.remove")).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.connect_event(Event::RemovePrompted(remove.clone()), tx);
            if let Some(o) = s.connect.open.as_mut() { o.menu = None; }
        }))
}

/// A connection that needs a fresh sign-in has its own link or signs in again.
fn choose_account(s: &mut App, a: &opencode_kit::integration::Connection, tx: &mut Tx) {
    tx.instant();
    match &a.status {
        Some(st) if st.url.is_some() => {
            let _ = tx.open_url(st.url.as_deref().unwrap_or_default());
        }
        Some(_) => s.connect_event(Event::AccountAdd, tx),
        None => s.connect_event(Event::ActivateRequested(a.id.clone()), tx),
    }
}

/// The models a new connection offers (`FirstConnectionModels`): one to start with, then
/// Continue in the footer.
fn models(s: &App, p: &Pal, id: &str) -> El {
    let Some(Step::Models { selected, collapsed }) = open(s).dialog.provider.as_ref().map(|p| &p.step) else { return col() };
    let groups = connection_groups(s, id);
    let row_for = |m: &crate::pickers::Model, last: bool| {
        let checked = *selected == m.key;
        let key = m.key.clone();
        let shell = col().key(format!("connected-model-{}", m.key)).mx(4.0).py(4.0)
            .child(row().role("radio").label(m.name.clone()).w_full().h(40.0).px(12.0).center_y().radius(4.0)
                .hover(p.layer3)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.connect_event(Event::ModelSelected(key.clone()), tx);
                })
                .child(text(m.name.clone()).font(crate::view::UI).color(p.text).grow(1.0).shrink().truncate())
                .child(el().size(16.0, 16.0).center().children(checked.then(|| icon(&ic::CHECK_SMALL, 14.0).color(p.icon)))));
        if last { shell } else { col().w_full().child(shell).child(el().h(0.5).mx(16.0).fill(p.border)) }
    };
    let list = |models: &[crate::pickers::Model]| {
        let n = models.len();
        col().key("connected-models").w_full().radius(8.0).fill(p.layer2).box_shadow(Pal::ring_in(p.border_muted)).clip_soft(0.0)
            .children(models.iter().enumerate().map(|(at, m)| row_for(m, at + 1 == n)))
    };
    let mut scroll = col().w_full().gap(12.0).pb(16.0)
        .child(row().key("available-models-heading").gap(6.0).center_y().child(text(opencode_i18n::t!("provider.connect.models.available")).font(LABEL).color(p.text)));
    if groups.len() == 1 {
        scroll = scroll.child(list(&groups[0].1));
    } else {
        for (info, models) in &groups {
            let open = !collapsed.contains(&info.id);
            let pid = info.id.clone();
            let mut section = col().w_full().gap(8.0).pb(if open { 8.0 } else { 0.0 })
                .child(row().key(format!("connect-group-{}", info.id)).role("button").h(28.0).pt(4.0).pr(8.0).pb(4.0).pl(4.0).gap(4.0).center_y().radius(4.0)
                    .hover(p.layer2)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        s.connect_event(Event::GroupCollapsed { provider: pid.clone(), collapsed: open }, tx);
                    })
                    .child(el().size(20.0, 20.0).center().child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }).mirror_in_rtl()))
                    .child(row().gap(8.0).center_y().child(widgets::provider_mark(&info.id, p.icon)).child(text(info.name.clone()).font(LABEL).color(p.text))));
            if open {
                section = section.child(list(models));
            }
            scroll = scroll.child(section);
        }
    }
    let can = groups.iter().any(|(_, ms)| ms.iter().any(|m| m.key == *selected));
    let pick = selected.clone();
    let footer = row().key("first-provider-model-footer").w_full().h(60.0).px(16.0).center_y()
        .child(spacer())
        .child(button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast).disabled(!can)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.connect_event(Event::ModelConfirmed(pick.clone()), tx)).build(kit(p)).key("connect-continue"));
    col().key("connect-body").w_full().grow(1.0).shrink().min_h(0.0)
        .child(col().w_full().grow(1.0).shrink().min_h(0.0).px(20.0)
            .child(text(opencode_i18n::t!("provider.connect.models.description")).font(BODY).color(p.muted).pb(20.0))
            // `settings-panel`: no bar, as Desktop hides it (`dialog.tsx:1241`, `settings.css:593-607`).
            .child(session_view::scroller::scroller("first-provider-model-scroll").hidden(col().w_full().grow(1.0).shrink().min_h(0.0), scroll)))
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(footer)
}

fn current(s: &App) -> &Conn {
    open(s).dialog.provider.as_ref().and_then(|p| p.step.connect()).unwrap_or(&Conn::Disposed)
}

fn provider_id(s: &App) -> String {
    open(s).dialog.provider.as_ref().map(|p| p.id.clone()).unwrap_or_default()
}

impl App {
    /// Whether the server shares this machine, so a sign-in stays here.
    pub(crate) fn server_is_local(&self) -> bool {
        match &*self.source {
            crate::Source::Live { server, .. } => server.is_local(),
            crate::Source::Fixture => true,
        }
    }
}

