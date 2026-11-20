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

mod carousel;
mod controls;
mod face;
pub mod local;
mod nodes;

use std::rc::Rc;

use opencode_widget::{Node, WidgetEvent};
use rune::{El, Image, Tx};
use rune_kit::Kit;

pub use face::{Words, frame, summary};
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
}

/// A lens from the host's state to a widget's [`Local`].
pub type Lens<S> = Rc<dyn for<'a> Fn(&'a mut S) -> &'a mut Local>;
/// Where a widget's events go.
pub type Emit<S> = Rc<dyn Fn(&mut S, WidgetEvent, &mut Tx)>;
/// Something the frame's buttons do (Try again).
pub type Action<S> = Rc<dyn Fn(&mut S, &mut Tx)>;

/// Where a widget's view sends what the user does, and where its [`Local`] lives in the host.
pub struct Hooks<S> {
    pub local: Lens<S>,
    pub emit: Emit<S>,
}

impl<S> Clone for Hooks<S> {
    fn clone(&self) -> Self {
        Hooks { local: self.local.clone(), emit: self.emit.clone() }
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

/// The widget's view tree: `live` when it takes input, else every control shows disabled.
/// Without `hooks` nothing in it responds, carousels included.
pub fn view<S: 'static>(look: &Look, local: &Local, hooks: Option<&Hooks<S>>, node: &Node, live: bool) -> El {
    let cx = Cx { look, local, hooks, live: live && hooks.is_some() };
    nodes::node(&cx, node, "w")
}
