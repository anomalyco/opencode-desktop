//! The widget's frame: what shows around (or instead of) its view in each lifecycle face —
//! a placeholder while the template loads, the live view with a failed step's error under it,
//! "Sending…" while the answer goes, the final view read-only with how it ended, or an error
//! card with Try again.


use opencode_widget::{Closed, Face};
use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Loader};
use serde_json::Value;

use crate::{Action, Buttons, Hooks, Local, Look, view};

/// The expand glyph (the diagram card's): two arrows out to the corners.
pub static EXPAND: rune::Icon = rune::Icon {
    name: "widget-expand",
    view: 16.0,
    body: r#"<path d="M9.5 2.5H13.5V6.5M13.5 2.5L9 7M6.5 13.5H2.5V9.5M2.5 13.5L7 9" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

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
    /// The expand button.
    pub fullscreen: String,
    /// Full screen's close button.
    pub exit_fullscreen: String,
    /// The inline copy while the widget is full screen.
    pub elsewhere: String,
    /// The hint while the widget has the keyboard.
    pub release: String,
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
            fullscreen: "Full screen".into(),
            exit_fullscreen: "Exit full screen".into(),
            elsewhere: "Open in full screen".into(),
            release: "Esc to release".into(),
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
/// `buttons` are what Try again (or Restart) and the expand button do.
pub fn frame<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, face: Face<'_>, title: &str, buttons: Buttons<S>) -> El {
    let Buttons { retry, expand } = buttons;
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
    if let (Some(h), Face::Live { live: true, .. }) = (hooks, face) {
        frame = keyboard(look, local, h, frame);
    }
    // Full screen, top right, on hover and keyboard focus, as a video player's.
    if let Some(expand) = expand {
        let words = look.words.fullscreen.clone();
        let button = el()
            .key("fullscreen")
            .size(25.0, 25.0)
            .radius(5.0)
            .center()
            .cursor(rune::Cursor::Pointer)
            .fill(c.bg_base)
            .border(0.5, c.border_weak)
            .hover(c.surface_weak)
            .group_opacity(0.0, 1.0)
            .role("button")
            .label(words.clone())
            .help(words)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| expand(s, tx))
            .child(icon(&EXPAND, 13.0).color(c.icon_muted));
        frame = frame.hover_group().hover_group_focus().child(button.pin(None, Some(10.0), Some(10.0), None));
    }
    frame
}

/// Room around a full-screen widget, each side.
const PAD: f32 = 24.0;

/// A widget that takes the keyboard (`widgets-input.md`): a click inside or Tab focuses it, keys
/// go to the host while it (or a control in it) has focus, a ring and "Esc to release" show
/// while it has, and the cursor hides while it holds the pointer captured.
fn keyboard<S: 'static>(look: &Look, local: &Local, hooks: &Hooks<S>, frame: El) -> El {
    let Some(input) = hooks.input.clone() else { return frame };
    let c = &look.kit.c;
    let (down, up, press) = (input.clone(), input.clone(), input);
    let mut frame = frame
        .focusable(true)
        .on_key(move |s: &mut S, ev, tx| down(s, crate::Input::Key { event: ev, up: false }, tx))
        .on_key_up(move |s: &mut S, ev, tx| up(s, crate::Input::Key { event: ev, up: true }, tx))
        .on_drag(move |s: &mut S, d: &mut rune::Drag, tx: &mut Tx| {
            if d.phase == rune::Phase::Down {
                press(s, crate::Input::Press, tx);
            }
        });
    if local.focused() {
        let hint = text(look.words.release.clone()).key("release").font(look.kit.ty.small.lh(16.0)).color(c.text_muted);
        frame = frame.child(hint.pin(None, None, Some(14.0), Some(6.0)));
    }
    if local.captured() {
        frame = frame.cursor(rune::Cursor::Hidden);
    }
    frame
}

/// The inline copy of a widget that is full screen: its frame, saying where it is.
pub fn placeholder(look: &Look, title: &str) -> El {
    let k = look.kit;
    let c = &k.c;
    col()
        .w_full()
        .max_w(look.width)
        .padding(16.0)
        .radius(14.0)
        .border(1.0, c.border_weak)
        .role("group")
        .label(title.to_owned())
        .child(row().key("elsewhere").gap(8.0).items_center().child(icon(&EXPAND, 13.0).color(c.icon_muted).a11y_hidden()).child(text(look.words.elsewhere.clone()).font(k.ty.small.lh(16.0)).color(c.text_muted)))
}

/// A widget full screen: its view fills the window, `height` points tall (scrolling when
/// taller), with a close button top right.
#[allow(clippy::too_many_arguments)]
pub fn screen<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, face: Face<'_>, title: &str, height: f32, close: Action<S>) -> El {
    local.built();
    let k = look.kit;
    let c = &k.c;
    let live = matches!(face, Face::Live { live: true, .. });
    let content = match face {
        Face::Live { view: v, live, error, .. } => {
            let mut b = vec![view(look, local, hooks, v, live).key("view").grow(1.0).min_h(0.0)];
            if let Some(e) = error {
                b.push(status(k, k.glyph.circle_x, c.danger.icon, e, None).key("error"));
            }
            b
        }
        Face::Closed { view: Some(v), .. } => vec![view(look, local, hooks, v, false).key("view")],
        _ => vec![row().key("loading").gap(8.0).items_center().child(Loader::new().size(14.0).build(k))],
    };
    let words = look.words.exit_fullscreen.clone();
    let close = el()
        .key("close")
        .size(32.0, 32.0)
        .radius(8.0)
        .center()
        .cursor(rune::Cursor::Pointer)
        .fill(c.bg_base)
        .border(0.5, c.border_weak)
        .hover(c.surface_weak)
        .role("button")
        .label(words.clone())
        .help(words)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| close(s, tx))
        .child(icon(k.glyph.close, 14.0).color(c.icon_base));
    let root = el().key("widget-screen");
    let root = match hooks {
        Some(h) if live => keyboard(look, local, h, root),
        _ => root,
    };
    root.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(c.bg_base)
        .role("dialog")
        .label(title.to_owned())
        .child(
            col()
                .key("scroll")
                .w_full()
                .h_full()
                .min_h(0.0)
                .scroll(rune::ScrollSpec { stick_bottom: false })
                .child(col().key("body").w_full().min_h(height.max(0.0)).padding(PAD).gap(12.0).children(content)),
        )
        .child(close.pin(None, Some(12.0), Some(12.0), None))
}
