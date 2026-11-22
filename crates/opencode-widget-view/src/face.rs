//! The widget's frame: what shows around (or instead of) its view in each lifecycle face —
//! a placeholder while the widget template loads, the live view with a failed step's error under it,
//! "Sending…" while the answer goes, the final view read-only with how it ended, or an error
//! card with Try again.


use opencode_widget::{Closed, Face};
use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, Kit, Loader};
use serde_json::Value;

use crate::{Action, Buttons, Hooks, Local, Look, Recorded, view};

/// The keyboard glyph of the focus hint.
static KEYBOARD: rune::Icon = rune::Icon {
    name: "widget-keyboard",
    view: 16.0,
    body: r#"<rect x="1.5" y="4" width="13" height="8" rx="1.5" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M4 6.5H4.5M6.5 6.5H7M9 6.5H9.5M11.5 6.5H12M5 9.5H11" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

/// The cursor glyph of the capture hint.
static CURSOR: rune::Icon = rune::Icon {
    name: "widget-cursor",
    view: 16.0,
    body: r#"<path d="M4 2.5L12 9H8.2L10 13L8.5 13.6L6.7 9.6L4 12V2.5Z" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round" fill="none"/>"#,
};

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
    /// A widget a newer one replaced.
    pub superseded: String,
    /// Sends a recorded program error to the agent.
    pub ask: String,
    pub asking: String,
    /// A recorded program error the agent heard of.
    pub told: String,
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
            ask: "Ask agent to fix it".into(),
            asking: "Asking the agent…".into(),
            told: "The agent was told about this error.".into(),
            release: "Esc to release".into(),
            superseded: "Updated below".into(),
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

/// The widget in its frame. `title` names it (the message's title, or its widget template's name);
/// `buttons` are what Try again (or Restart) and the expand button do.
pub fn frame<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, face: Face<'_>, title: &str, buttons: Buttons<S>) -> El {
    let Buttons { retry, expand, failure } = buttons;
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
                Closed::Superseded => status(k, k.glyph.chevron_down, c.icon_muted, &look.words.superseded, None),
            });
            b
        }
    };
    // A program error the server recorded (here or on another client): what it was, and the agent
    // told of it or the user's button to tell it.
    let body: Vec<El> = body.into_iter().chain(failure.map(|f| recorded(look, f))).collect();
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

/// A recorded program error's rows: its words (unless the widget shows its own error), then "The
/// agent was told about this error." or "Ask agent to fix it".
fn recorded<S: 'static>(look: &Look, f: Recorded<S>) -> El {
    let k = look.kit;
    let c = &k.c;
    let words = f.text.map(|t| text(t).key("words").font(k.ty.small.lh(16.0)).color(c.danger.fg).wrap().selectable(true).w_full());
    let action = if f.sent {
        text(look.words.told.clone()).key("told").font(k.ty.small.lh(16.0)).color(c.text_muted)
    } else {
        let label = if f.ask.is_some() { look.words.ask.clone() } else { look.words.asking.clone() };
        let button = Button::new(label).variant(ButtonVariant::Neutral);
        let button = match f.ask {
            Some(ask) => button.on_press_tx(move |s: &mut S, tx: &mut Tx| ask(s, tx)),
            None => button.state(rune_kit::State::Disabled),
        };
        row().key("ask").child(button.build(k))
    };
    col().key("failure").w_full().gap(8.0).role("status").children(words).child(action)
}

/// The full-screen close button: `CLOSE` points square, `CLOSE_INSET` from the window's top and
/// right edge, all of it visible (`widgets-fullscreen.md`, Chrome).
pub const CLOSE: f32 = 32.0;
pub const CLOSE_INSET: f32 = 8.0;
/// Full screen, the root sits `PAD` in from every edge of the window (canvases too); the button
/// keeps its place on screen, over the padded root's top-right corner.
pub const PAD: f32 = 16.0;

/// The close button's area `[x, y, w, h]` over a full-screen widget laid out at `screen`
/// (`[x, y, w, h]`), in the same coordinates.
pub fn close_area([x, y, w, _]: [f32; 4]) -> [f32; 4] {
    [x + w - CLOSE_INSET - CLOSE, y + CLOSE_INSET, CLOSE, CLOSE]
}

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
    // A tiny hint: what the widget holds (the keyboard, or the pointer too) and the key that lets go.
    if local.focused() {
        let glyph = if local.captured() { &CURSOR } else { &KEYBOARD };
        let hint = row()
            .key("release")
            .gap(3.0)
            .items_center()
            .role("status")
            .label(look.words.release.clone())
            .child(icon(glyph, 12.0).color(c.icon_muted).a11y_hidden())
            .child(text("Esc").font(look.kit.ty.small.lh(14.0)).color(c.text_muted).a11y_hidden());
        frame = frame.child(hint.pin(None, None, Some(12.0), Some(6.0)));
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

/// A widget full screen: its view fills the window less [`PAD`] each side, `height` points tall
/// (scrolling when taller), the close button floating over its top-right corner ([`close_area`]). Esc, back and
/// the mouse's back button close it as the button does (the host takes the keys).
#[allow(clippy::too_many_arguments)]
pub fn screen<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, face: Face<'_>, title: &str, height: f32, close: Action<S>) -> El {
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
    // No title bar: only the close button floating over the top-right corner.
    let back = close.clone();
    let close = el()
        .key("close")
        .size(CLOSE, CLOSE)
        .radius(CLOSE / 2.0)
        .center()
        .cursor(rune::Cursor::Pointer)
        .fill(c.bg_base.alpha(0.7))
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
        .pointer_buttons([rune::PointerButton::Back])
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            if tx.button() == rune::PointerButton::Back {
                back(s, tx);
            }
        })
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
        .child(close.pin(None, Some(CLOSE_INSET), Some(CLOSE_INSET), None))
}
