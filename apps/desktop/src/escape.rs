//! Escape closes the topmost layer and nothing else, as Desktop's layer stacks do: its popover
//! stack dismisses only the top entry on `escape.pressed` (`ui/src/components/popover-machine.ts`
//! `transitionPopoverStack`) and its dialog stack only the top dialog on `dismiss`
//! (`ui/src/context/dialog-machine.ts`). With no layer open Escape does nothing: it never
//! leaves a session or takes focus out of the composer.
//!
//! Menus, pickers, the palette and Rune modals take Escape in their own keys first; this stack
//! holds the layers the page opens over the session. [`stack`] reads it from the app, bottom
//! first, [`step`] picks what Escape does, and [`apply`] carries it out.

use rune::{KeyEvent, Tx};

use crate::{App, Route};

/// A layer Escape can close, from the bottom of the stack up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layer {
    /// A subagent's page with nothing typed: Escape goes back to the session that started it
    /// (`session/composer/region.tsx` `openParent`).
    ChildSession,
    /// A tool step opened in the side region.
    ToolStep,
    /// A toast that stays until dismissed.
    Toast,
    Find,
    Todo,
    Activity,
    HomeMenu,
    /// The open session's run, while the composer has focus: Escape stops it.
    Run,
    /// A session still opening.
    Opening,
    /// The "Add to chat" offer over a selection.
    Quote,
    /// The sidebar peeking over the page.
    Peek,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Pressed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Dismiss(Layer),
}

/// Escape dismisses the top layer only.
pub(crate) fn step(stack: &[Layer], event: Event) -> Option<Effect> {
    match event {
        Event::Pressed => stack.last().copied().map(Effect::Dismiss),
    }
}

/// The open layers, bottom first.
pub(crate) fn stack(s: &App) -> Vec<Layer> {
    let session = matches!(s.route, Route::Session(_));
    [
        (Layer::ChildSession, crate::subagents::reply::composer_empty(s) && s.has_parent()),
        (Layer::ToolStep, s.tool_view_open()),
        (Layer::Toast, s.toast.current.as_ref().is_some_and(|t| t.persistent)),
        (Layer::Find, s.find.open && session),
        (Layer::Todo, s.todo_open),
        (Layer::Activity, s.shells.activity.is_open()),
        (Layer::HomeMenu, s.project_state.home_menu_open),
        (Layer::Run, crate::stop::escape_stops(s)),
        (Layer::Opening, s.opening.is_some()),
        (Layer::Quote, s.quote.offered()),
        (Layer::Peek, !s.sidebar_open && s.layout.peek.shown()),
    ]
    .into_iter()
    .filter_map(|(layer, open)| open.then_some(layer))
    .collect()
}

/// Escape in the window: true when a layer took it.
pub(crate) fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(effect) = step(&stack(s), Event::Pressed) else { return false };
    apply(s, effect, ev, tx);
    true
}

fn apply(s: &mut App, effect: Effect, ev: &KeyEvent, tx: &mut Tx) {
    let Effect::Dismiss(layer) = effect;
    match layer {
        Layer::Peek => s.queue_peek(crate::layout_state::PeekEvent::Escape, tx),
        Layer::Quote => {
            tx.instant();
            crate::quote::dismiss(s, tx);
        }
        Layer::Opening => s.cancel_open(tx),
        Layer::Run => {
            crate::stop::key(s, ev, tx);
        }
        Layer::HomeMenu => {
            tx.instant();
            s.project_state.home_menu_open = false;
        }
        Layer::Activity => {
            tx.instant();
            crate::subagents::shells::set_open(s, false);
        }
        Layer::Todo => {
            tx.instant();
            s.todo_open = false;
        }
        Layer::Find => {
            crate::find::dismiss_key(s, ev, tx);
        }
        Layer::Toast => {
            crate::status_toast::key(s, ev, tx);
        }
        Layer::ToolStep => {
            s.dismiss_tool_step(tx);
        }
        Layer::ChildSession => {
            s.open_parent(tx);
        }
    }
}
