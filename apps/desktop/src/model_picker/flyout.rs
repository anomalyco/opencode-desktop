//! The lit model's efforts, beside its row in the model list (Desktop's flyout in
//! `select-dialog.tsx`): resting on the row opens it ([`super::hover`]), as does a press on
//! the row's "Xhigh ›", and picking one picks the model with that effort. It appears at once,
//! like a submenu: just past the list's edge, beside the "Xhigh ›" at the row's end, with the
//! checked effort level with the row.

use rune::{Align, El, Side, Transition, Tx, col, el};

use crate::App;
use crate::palette::Pal;
use crate::pickers::Kind;
use crate::popover::{self as pop, MENU, Opt};
use opencode_kit::model::capitalize;

/// The open flyout: the model it belongs to, and the effort the pointer lights in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flyout {
    pub model: String,
    pub lit: Option<String>,
}

impl App {
    /// A press on the row's "Xhigh ›": its efforts open beside it, or stay open when the
    /// pointer's rest already opened them.
    pub fn press_effort_chevron(&mut self, key: &str) {
        self.pickers.activate(key.to_owned());
        self.pickers.hover.cancel_wait();
        if self.model_effort(key).is_some() { self.open_effort_flyout(key); }
        self.pickers.search.focused = true;
    }

    /// Opens the model `key`'s efforts beside its row, or closes them when they're open.
    pub fn toggle_effort_flyout(&mut self, key: &str) {
        if self.pickers.flyout.as_ref().is_some_and(|f| f.model == key) {
            self.pickers.flyout = None;
        } else {
            self.press_effort_chevron(key);
        }
    }

    /// Picks the model `key` with `effort` ("" for its default).
    pub fn choose_effort(&mut self, key: &str, effort: &str, tx: &mut Tx) {
        self.pickers.stage_effort(key, effort);
        self.pickers.flyout = None;
        self.choose_picker_tx(key, tx);
    }
}

/// The open flyout's model, its efforts with Default first, and the one it would run with.
fn efforts<'a>(s: &'a App) -> Option<(&'a Flyout, Vec<String>, String)> {
    let f = s.pickers.flyout.as_ref().filter(|_| s.pickers.open == Some(Kind::Model))?;
    let model = s.pickers.models.iter().find(|m| m.key == f.model)?;
    let now = s.model_effort(&f.model)?;
    Some((f, std::iter::once(String::new()).chain(model.variants.iter().cloned()).collect(), now))
}

/// What the flyout hangs from, inside the model row `key` while its efforts are open: a line
/// across the row, raised so the flyout's checked effort lands level with the row. The window
/// still keeps the flyout inside it.
pub fn anchor(s: &App, key: &str) -> Option<El> {
    let (_, all, now) = efforts(s).filter(|(f, ..)| f.model == key)?;
    let at = all.iter().position(|e| *e == now).unwrap_or(0) as f32;
    let up = MENU.inset + at * MENU.row_h + (MENU.row_h - pop::LIST.row_h) / 2.0;
    Some(el().key("effort-anchor").pin(Some(0.0), Some(-up), Some(0.0), None).h(0.0).transition(Transition::none()))
}

/// The flyout, when one is open: Default and each effort, the one the model would run with
/// checked.
pub fn view(s: &App, p: &Pal) -> Option<El> {
    let (f, all, now) = efforts(s)?;
    let mut list = col().key("effort-options").w_full().padding(MENU.inset);
    for effort in all {
        let label = if effort.is_empty() { "Default".to_owned() } else { capitalize(&effort) };
        let id = if effort.is_empty() { "default".to_owned() } else { effort.clone() };
        let o = Opt::new(format!("effort-{id}"), label).checked(effort == now).active(f.lit.as_deref() == Some(id.as_str()));
        let (key, lit) = (f.model.clone(), id.clone());
        let row = pop::option(p, MENU, o, move |s: &mut App| {
            if let Some(f) = &mut s.pickers.flyout { f.lit = Some(lit.clone()); }
        }, move |s: &mut App, tx: &mut Tx| s.choose_effort(&key, &effort, tx));
        list = list.child(row);
    }
    let menu = super::hover::flyout(pop::surface(p, MENU, "effort-flyout", "Choose effort").w(160.0).child(list));
    // 2 pt past the list's edge.
    Some(menu.anchored_fit("effort-anchor", Side::Right, Align::Start, pop::LIST.inset + 2.0))
}
