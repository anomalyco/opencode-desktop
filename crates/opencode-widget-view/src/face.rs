//! The widget's frame: what shows around (or instead of) its view in each lifecycle face —
//! a placeholder while the template loads, the live view with a failed step's error under it,
//! "Sending…" while the answer goes, the final view read-only with how it ended, or an error
//! card with Try again.


use opencode_widget::{Closed, Face};
use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Loader};
use serde_json::Value;

use crate::{Action, Hooks, Local, Look, view};

/// The frame's words, in the interface's language.
#[derive(Clone, Debug)]
pub struct Words {
    pub loading: String,
    pub failed: String,
    pub retry: String,
    /// Starts a live widget that stopped again.
    pub restart: String,
    pub sending: String,
    pub submitted: String,
    pub dismissed: String,
    /// A carousel's arrows.
    pub previous: String,
    pub next: String,
}

impl Default for Words {
    fn default() -> Self {
        Words {
            loading: "Loading widget…".into(),
            failed: "This widget couldn't be shown".into(),
            retry: "Try again".into(),
            restart: "Restart".into(),
            sending: "Sending…".into(),
            submitted: "Submitted".into(),
            dismissed: "Dismissed".into(),
            previous: "Previous".into(),
            next: "Next".into(),
        }
    }
}

/// A submitted value in a line: an object's fields as `key: value`, lists joined, at most ~160
/// characters.
pub fn summary(v: &Value) -> String {
    fn one(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Array(a) => a.iter().map(one).collect::<Vec<_>>().join(", "),
            Value::Null => "—".into(),
            other => other.to_string(),
        }
    }
    let s = match v {
        Value::Object(m) => m.iter().map(|(k, v)| format!("{k}: {}", one(v))).collect::<Vec<_>>().join(" · "),
        other => one(other),
    };
    if s.chars().count() > 160 { s.chars().take(159).collect::<String>() + "…" } else { s }
}

fn status(k: &Kit, glyph: &'static rune::Icon, tint: rune::Color, words: &str, detail: Option<String>) -> El {
    row()
        .key("status")
        .w_full()
        .gap(6.0)
        .items_center()
        .child(icon(glyph, 12.0).key("icon").color(tint).a11y_hidden())
        .child(text(words.to_owned()).key("words").font(k.ty.small_medium.lh(16.0)).color(k.c.text_base))
        .children(detail.map(|d| text(d).key("detail").font(k.ty.small.lh(16.0)).color(k.c.text_muted).truncate().grow(1.0).min_w(0.0)))
        .role("status")
}

/// The widget in its frame. `title` names it (the message's title, or its template's name);
/// `retry` is what Try again does on a failed one.
pub fn frame<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, face: Face<'_>, title: &str, retry: Option<Action<S>>) -> El {
    local.built();
    let k = look.kit;
    let c = &k.c;
    let body: Vec<El> = match face {
        Face::Loading => vec![
            row().key("loading").gap(8.0).items_center().child(Loader::new().size(14.0).build(k)).child(text(look.words.loading.clone()).font(k.ty.small.lh(16.0)).color(c.text_muted)),
            el().key("skeleton").w_full().h(96.0).radius(10.0).fill(c.surface_weak),
        ],
        Face::Failed(message) => {
            let mut b = vec![
                status(k, k.glyph.circle_x, c.danger.icon, &look.words.failed, None),
                text(message.to_owned()).key("message").font(k.ty.mono).color(c.text_muted).wrap().selectable(true).w_full(),
            ];
            if let Some(retry) = retry {
                b.push(row().key("actions").child(Button::new(look.words.retry.clone()).variant(ButtonVariant::Neutral).on_press_tx(move |s: &mut S, tx: &mut Tx| retry(s, tx)).build(k)));
            }
            b
        }
        Face::Live { view: v, live, sending, error, restart } => {
            let mut b = vec![view(look, local, hooks, v, live).key("view")];
            if let Some(e) = error {
                b.push(status(k, k.glyph.circle_x, c.danger.icon, e, None).key("error"));
            }
            if let (true, Some(retry)) = (restart, retry) {
                b.push(row().key("actions").child(Button::new(look.words.restart.clone()).variant(ButtonVariant::Neutral).on_press_tx(move |s: &mut S, tx: &mut Tx| retry(s, tx)).build(k)));
            }
            if sending {
                b.push(row().key("sending").gap(8.0).items_center().child(Loader::new().size(12.0).build(k)).child(text(look.words.sending.clone()).font(k.ty.small.lh(16.0)).color(c.text_muted)));
            }
            b
        }
        Face::Closed { view: v, how } => {
            let mut b: Vec<El> = v.map(|v| view(look, local, hooks, v, false).key("view")).into_iter().collect();
            b.push(match how {
                Closed::Submitted(value) => status(k, k.glyph.check, c.success.icon, &look.words.submitted, Some(summary(value)).filter(|s| !s.is_empty())),
                Closed::Dismissed => status(k, k.glyph.close, c.icon_muted, &look.words.dismissed, None),
            });
            b
        }
    };
    // What the widget may do beyond drawing, granted (a live widget's `fetch`): "Network: api.github.com".
    let chip = look.chip.map(|chip| {
        row().key("capabilities").child(
            text(chip.to_owned()).key("chip").font(k.ty.small.lh(16.0)).color(c.text_muted).truncate().px(8.0).py(2.0).radius(8.0).border(1.0, c.border_weak),
        )
    });
    let mut frame = col()
        .w_full()
        .max_w(look.width)
        .gap(12.0)
        .padding(16.0)
        .radius(14.0)
        .border(1.0, c.border_weak)
        .role("group")
        .label(title.to_owned())
        .children(chip)
        .children(body);
    // Whether it shows: a live widget runs only while it does.
    if let Some(h) = hooks {
        let lens = h.local.clone();
        frame = frame.on_visible(move |s: &mut S, on, _tx: &mut Tx| (lens)(s).set_shown(on));
    }
    frame
}
