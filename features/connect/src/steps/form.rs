//! Interactive method-form steps: external links, multiselect checkboxes, single-choice radios
//! (with optional custom input), and text/number inputs.

use opencode_kit::integration::FieldKind;
use rune::{El, TextEdit, Tx, col, row, text};
use rune_kit::{ButtonVariant, Checkbox, RadioItem, TextInput};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};

use super::{auth, current, input};
use crate::form::{self, FieldError, Issue};
use crate::machine::{Event as Input, State as Conn};
use crate::ui::{self, BODY, BODY_MED};
use crate::ConnectHost;

fn form_input(host: &mut dyn ConnectHost, i: form::Input, tx: &mut Tx) {
    input(host, Input::Form(i), tx);
}

fn submit_continue(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let pre = host.connect().dialog().and_then(|o| {
        let Conn::Form(_, f) = o.dialog.provider.as_ref()?.step.connect()? else { return None };
        match f.step.kind {
            form::Kind::Text { .. } => Some(form::Input::TextChanged(o.field.text.clone())),
            form::Kind::Choice { choice: form::Choice::Custom { .. }, .. } => Some(form::Input::CustomChanged(o.field.text.clone())),
            _ => None,
        }
    });
    if let Some(ev) = pre {
        form_input(host, ev, tx);
    }
    form_input(host, form::Input::Submitted, tx);
}

fn add_custom(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let text = host.connect().dialog().map(|o| o.field.text.clone()).unwrap_or_default();
    form_input(host, form::Input::CustomChanged(text), tx);
    form_input(host, form::Input::CustomAdded, tx);
    if let Some(o) = host.connect_mut().dialog_mut() {
        o.field = TextEdit::new("");
        o.field.focused = true;
    }
}

fn sync_custom_field(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let text = host.connect().dialog().map(|o| o.field.text.clone()).unwrap_or_default();
    form_input(host, form::Input::CustomChanged(text), tx);
}

fn sync_text_field(host: &mut dyn ConnectHost, tx: &mut Tx) {
    let text = host.connect().dialog().map(|o| o.field.text.clone()).unwrap_or_default();
    form_input(host, form::Input::TextChanged(text), tx);
}

pub(super) fn view<H: ConnectHost + 'static>(s: &H, p: &Pal) -> El {
    let Conn::Form(_, f) = current(s) else { return col() };
    let controls = match &f.step.kind {
        form::Kind::External { url, browser } => external::<H>(p, f, url, browser),
        form::Kind::Multiselect { picked, custom } => multiselect(s, p, f, picked, custom),
        form::Kind::Choice { .. } => choice(s, p, f),
        form::Kind::Text { .. } => text_step(s, p, f),
    };
    build_form_layout(p, f, controls)
}

fn build_form_layout(p: &Pal, f: &form::Form, controls: El) -> El {
    let mut head = col().w_full().gap(4.0).child(text(f.title()).font(BODY_MED).color(p.text).wrap());
    if let Some(d) = &f.step.field.description {
        head = head.child(text(d.clone()).font(BODY).color(p.muted).wrap());
    }
    let mut e = col().key(format!("connect-form-{}", f.key())).px(12.0).gap(16.0).child(head).child(controls);
    if let Some(err) = f.error() {
        let words = if err == FieldError::Required {
            opencode_i18n::t!("provider.connect.form.required")
        } else {
            opencode_i18n::t!("provider.connect.form.number")
        };
        e = e.child(text(words).key("connect-form-error").role("alert").font(BODY).color(p.danger));
    }
    e
}

fn cont<H: ConnectHost + 'static>(p: &Pal, enabled: bool) -> El {
    ui::button(opencode_i18n::t!("common.continue"), ButtonVariant::Contrast)
        .disabled(!enabled)
        .on_press_tx(|s: &mut H, tx: &mut Tx| submit_continue(s, tx))
        .build(kit(p))
        .key("connect-continue")
}

fn external<H: ConnectHost + 'static>(p: &Pal, f: &form::Form, url: &str, browser: &form::Browser) -> El {
    let opening = matches!(browser, form::Browser::Opening { .. });
    let label = opencode_i18n::t!("provider.connect.form.open");
    let mut e = col().w_full().gap(16.0).child(
        row()
            .gap(8.0)
            .child(
                ui::button(label, ButtonVariant::Neutral)
                    .icon(&ic::ARROW_UP_RIGHT)
                    .disabled(opening)
                    .on_press_tx(|s: &mut H, tx: &mut Tx| form_input(s, form::Input::ExternalOpen, tx))
                    .build(kit(p))
                    .key("connect-open-link"),
            )
            .child(
                ui::button(opencode_i18n::t!("provider.connect.form.finished"), ButtonVariant::Contrast)
                    .disabled(!f.can_finish())
                    .on_press_tx(|s: &mut H, tx: &mut Tx| form_input(s, form::Input::Submitted, tx))
                    .build(kit(p))
                    .key("connect-finished"),
            ),
    );
    if *browser == form::Browser::Failed {
        e = e
            .child(text(opencode_i18n::t!("provider.connect.form.browserFailed")).role("alert").font(BODY).color(p.danger))
            .child(auth::copy_field::<H>(p, "connect-link", None, None, url.to_owned()));
    }
    e
}

fn multiselect<H: ConnectHost + 'static>(s: &H, p: &Pal, f: &form::Form, picked: &[String], _custom: &str) -> El {
    let Some(open) = s.connect().dialog() else { return col() };
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
        boxes = boxes.child(
            c.on_toggle_tx(move |s: &mut H, tx: &mut Tx| form_input(s, form::Input::OptionToggled(value.clone(), !checked), tx))
                .build(kit(p)),
        );
    }
    let mut e = col().w_full().gap(16.0).child(boxes);
    if f.step.field.custom {
        let field = TextInput::new(&open.field)
            .placeholder(opencode_i18n::t!("rune.connect.form.placeholder"))
            .label(opencode_i18n::t!("ui.messagePart.option.typeOwnAnswer"))
            .width(None)
            .spellcheck(s.spellcheck())
            .on_edit(|s: &mut H| s.connect_mut().field_mut())
            .on_submit(|s: &mut H, tx: &mut Tx| add_custom(s, tx))
            .build(kit(p))
            .grow(1.0)
            .shrink();
        e = e.child(
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(field)
                .child(ui::button(opencode_i18n::t!("provider.connect.form.add"), ButtonVariant::Neutral).on_press_tx(|s: &mut H, tx: &mut Tx| add_custom(s, tx)).build(kit(p))),
        );
    }
    if let Some(issue) = f.issue() {
        let words = match issue {
            Issue::SelectOne => opencode_i18n::t!("provider.connect.form.selectOne").to_owned(),
            Issue::MinItems => opencode_i18n::t!("provider.connect.form.minItems", count = f.step.field.min_items.unwrap_or(0)),
            Issue::MaxItems => opencode_i18n::t!("provider.connect.form.maxItems", count = f.step.field.max_items.unwrap_or(0)),
        };
        e = e.child(text(words).font(BODY).color(p.muted));
    }
    e.child(row().child(cont::<H>(p, f.can_continue())))
}

fn choice<H: ConnectHost + 'static>(s: &H, p: &Pal, f: &form::Form) -> El {
    let Some(open) = s.connect().dialog() else { return col() };
    let selected = f.selected();
    let mut radios = col().key("connect-radios").w_full().gap(12.0);
    for (value, label, description) in f.options() {
        let mut r = RadioItem::new(label, selected.as_deref() == Some(value.as_str()));
        if let Some(d) = description {
            r = r.description(d);
        }
        let key = format!("connect-radio-{value}");
        radios = radios.child(
            r.on_select_tx(move |s: &mut H, tx: &mut Tx| form_input(s, form::Input::ChoiceSelected(value.clone()), tx))
                .build(kit(p))
                .key(key),
        );
    }
    if f.step.field.kind == FieldKind::String && f.step.field.custom {
        let custom = selected.as_deref() == Some("custom");
        radios = radios.child(
            RadioItem::new(opencode_i18n::t!("ui.messagePart.option.typeOwnAnswer"), custom)
                .on_select_tx(|s: &mut H, tx: &mut Tx| form_input(s, form::Input::ChoiceSelected("custom".into()), tx))
                .build(kit(p)),
        );
        if custom {
            let placeholder = f.step.field.placeholder.clone().unwrap_or_else(|| opencode_i18n::t!("rune.connect.form.placeholder").into());
            radios = radios.child(
                col().pl(24.0).child(
                    TextInput::new(&open.field)
                        .placeholder(placeholder)
                        .label(opencode_i18n::t!("ui.messagePart.option.typeOwnAnswer"))
                        .width(None)
                        .spellcheck(s.spellcheck())
                        .on_edit(|s: &mut H| s.connect_mut().field_mut())
                        .on_change(|s: &mut H, tx: &mut Tx| sync_custom_field(s, tx))
                        .build(kit(p)),
                ),
            );
        }
    }
    let mut actions = row().gap(8.0).center_y().child(cont::<H>(p, f.can_continue()));
    if !f.step.field.required {
        actions = actions.child(
            ui::button(opencode_i18n::t!("provider.connect.form.skip"), ButtonVariant::GhostMuted)
                .on_press_tx(|s: &mut H, tx: &mut Tx| form_input(s, form::Input::Skipped, tx))
                .build(kit(p)),
        );
    }
    col().w_full().gap(16.0).child(radios).child(actions)
}

fn text_step<H: ConnectHost + 'static>(s: &H, p: &Pal, f: &form::Form) -> El {
    let Some(open) = s.connect().dialog() else { return col() };
    let field = TextInput::new(&open.field)
        .placeholder(f.step.field.placeholder.clone().unwrap_or_default())
        .label(f.title())
        .width(None)
        .invalid(f.error().is_some())
        .on_edit(|s: &mut H| s.connect_mut().field_mut())
        .on_change(|s: &mut H, tx: &mut Tx| sync_text_field(s, tx))
        .on_submit(|s: &mut H, tx: &mut Tx| form_input(s, form::Input::Submitted, tx))
        .build(kit(p))
        .key("connect-field");
    col().w_full().gap(16.0).child(field).child(row().child(cont::<H>(p, true)))
}
