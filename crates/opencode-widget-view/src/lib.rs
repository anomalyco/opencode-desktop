//! Draws an agent widget (`opencode-widget`) with Rune: the node tree a step returns, with
//! rune-kit's controls and the host's Kit tokens, inside a widget frame that shows its
//! lifecycle (loading, live, sending, submitted or dismissed, failed).
//!
//! It knows nothing of sessions, windows or servers. The host supplies:
//! - a [`Look`]: the Kit, how to draw Markdown and how to find a picture for an image `src`
//!   (fetching and decoding stay the host's, cached once per source);
//! - a [`Hooks`]: a lens to the widget's [`Local`] state and where the widget's events go.
//!
//! The view allocates no GPU resources of its own and holds nothing per frame: every build reads
//! the widget's view, its [`Local`] and the pictures the host already holds.

#![forbid(unsafe_code)]

pub mod canvas;
mod carousel;
mod controls;
mod face;
pub mod local;
mod nodes;
pub mod shader;

use std::rc::Rc;

use opencode_widget::{Node, WidgetEvent};
use rune::{El, Image, Tx};
use rune_kit::Kit;

pub use face::{CLOSE, CLOSE_INSET, EXPAND, PAD, Words, close_area, frame, placeholder, screen, summary};

/// The button a press used, as DOM `button` numbers it.
pub(crate) fn button(tx: &Tx) -> u8 {
    match tx.button() {
        rune::PointerButton::Primary => 0,
        rune::PointerButton::Middle => 1,
        _ => 2,
    }
}

/// A canvas or shader heard the pointer at `at`: with the pointer captured it goes to the host
/// as captured input (true), and a press takes focus first.
pub(crate) fn captured<S: 'static>(hooks: &Hooks<S>, s: &mut S, id: Option<&str>, kind: opencode_widget::PointerKind, at: [f32; 2], tx: &mut Tx) -> bool {
    let Some(input) = hooks.input.clone() else { return false };
    if !(hooks.local)(s).captured() {
        if kind == opencode_widget::PointerKind::Down {
            input(s, Input::Press, tx);
        }
        return (hooks.local)(s).captured() && kind == opencode_widget::PointerKind::Down;
    }
    input(s, Input::Captured { id, kind, at, button: button(tx) }, tx)
}
pub use local::{DEBOUNCE_S, Local};

/// What a widget's build reads from the host besides the widget.
pub struct Look<'a> {
    pub kit: &'a Kit,
    /// Markdown as the host draws it (links, code, its own type scale).
    pub markdown: &'a dyn Fn(&str) -> El,
    /// The picture for image `src` once the host has it fetching or decoded; `None` while it
    /// isn't, or when it can't be had.
    pub image: &'a dyn Fn(&str) -> Option<Image>,
    /// The widget frame's words, in the interface's language.
    pub words: &'a Words,
    /// Room the widget may take, in points: rows of cards fall to a column below it.
    pub width: f32,
    /// Device pixels per point: how finely canvas curves tessellate.
    pub scale: f32,
    /// The theme is dark (a canvas's `frame.dark`).
    pub dark: bool,
    /// What the widget is to the host (its message id): keeps each shader slot apart.
    pub scope: &'a str,
    /// The capabilities granted to it, in a line ("Network: api.github.com"), when it has any.
    pub chip: Option<&'a str>,
    /// It fills the window (full screen): its root fills it, and a `fill` canvas or shader takes
    /// the height left.
    pub fullscreen: bool,
}

/// A lens from the host's state to a widget's [`Local`].
pub type Lens<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut Local>;
/// Where a widget's events go.
pub type Emit<S> = Rc<dyn Fn(&mut S, WidgetEvent, &mut Tx)>;
/// Something the frame's buttons do (Try again).
pub type Action<S> = Rc<dyn Fn(&mut S, &mut Tx)>;

/// What the frame's own buttons do; a button without an action doesn't show.
pub struct Buttons<S> {
    /// Try again on a failed widget, Restart on a stopped live one.
    pub retry: Option<Action<S>>,
    /// The expand button, top right, on hover and keyboard focus: full screen.
    pub expand: Option<Action<S>>,
    /// A program error the server recorded for the widget (`State.failure`).
    pub failure: Option<Recorded<S>>,
}

impl<S> Default for Buttons<S> {
    fn default() -> Self {
        Buttons { retry: None, expand: None, failure: None }
    }
}

/// A program error the server recorded for an open widget (`widgets.md`, Failures), with what
/// the user can do about it.
pub struct Recorded<S> {
    /// "Failed on ios: event: … (12:5)"; none when the widget shows its own error already.
    pub text: Option<String>,
    /// The agent was told about it.
    pub sent: bool,
    /// "Ask agent to fix it"; none while the ask is on its way.
    pub ask: Option<Action<S>>,
}

/// Input the host's keyboard focus and pointer capture take (`widgets-input.md`), before it
/// reaches the widget template.
pub enum Input<'a> {
    /// A key pressed (`up`: released) while the widget or a control in it has focus.
    Key { event: &'a rune::KeyEvent, up: bool },
    /// A press inside the widget: it takes focus, and a capture it asked for.
    Press,
    /// The pointer while captured, at `at` (logical points in the element that heard it):
    /// moving, or a button going down or up.
    Captured { id: Option<&'a str>, kind: opencode_widget::PointerKind, at: [f32; 2], button: u8 },
}

/// Where the host takes [`Input`]; true when it took it.
pub type InputFn<S> = Rc<dyn Fn(&mut S, Input<'_>, &mut Tx) -> bool>;

/// Where a widget's view sends what the user does, and where its [`Local`] lives in the host.
pub struct Hooks<S> {
    pub local: Lens<S>,
    pub emit: Emit<S>,
    /// Keyboard focus and pointer capture, when the host offers them.
    pub input: Option<InputFn<S>>,
}

impl<S> Clone for Hooks<S> {
    fn clone(&self) -> Self {
        Hooks { local: self.local.clone(), emit: self.emit.clone(), input: self.input.clone() }
    }
}

impl<S: 'static> Hooks<S> {
    /// Sends `event`, after any typing not yet committed, so its step sees what was typed.
    pub fn send(&self, s: &mut S, event: WidgetEvent, tx: &mut Tx) {
        for pending in (self.local)(s).flush() {
            (self.emit)(s, pending, tx);
        }
        (self.emit)(s, event, tx);
    }
}

/// One build's context: what it reads, where its handlers go, and whether the view takes input.
pub(crate) struct Cx<'a, S> {
    pub look: &'a Look<'a>,
    pub local: &'a Local,
    pub hooks: Option<&'a Hooks<S>>,
    /// The view takes input (never without hooks).
    pub live: bool,
}

impl<S> Cx<'_, S> {
    /// The hooks, when the view takes input.
    pub fn act(&self) -> Option<Hooks<S>> {
        self.hooks.filter(|_| self.live).cloned()
    }
}

/// Points for a gap unit (the contract's 0–3; GUI: 0/4/8/12). Absent, 8.
pub(crate) fn gap(g: Option<u8>) -> f32 {
    match g {
        Some(0) => 0.0,
        Some(1) => 4.0,
        Some(2) | None => 8.0,
        Some(_) => 12.0,
    }
}

thread_local! {
    static APPEARANCE: std::cell::Cell<(f32, bool)> = const { std::cell::Cell::new((2.0, true)) };
}

/// The device scale and darkness the last widget built with, for the host's draw steps
/// (`frame.scale`, `frame.dark`), which run outside a build.
pub fn appearance() -> (f32, bool) {
    APPEARANCE.get()
}

/// Whether a canvas or shader `fill` grows in full screen: by default, unless `fill: false`.
pub(crate) fn grows(fill: Option<bool>) -> bool {
    fill != Some(false)
}

/// Whether node `n` holds a canvas or shader that grows into the height left in full screen.
pub(crate) fn fills(n: &Node) -> bool {
    let mut any = false;
    n.walk(&mut |n| any |= matches!(n, Node::Canvas { fill, .. } | Node::Shader { fill, .. } if grows(*fill)));
    any
}

/// The widget's view tree: `live` when it takes input, else every control shows disabled.
/// Without `hooks` nothing in it responds, carousels included.
pub fn view<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, node: &Node, live: bool) -> El {
    APPEARANCE.set((look.scale, look.dark));
    let cx = Cx { look, local, hooks, live: live && hooks.is_some() };
    nodes::node(&cx, node, "w")
}
