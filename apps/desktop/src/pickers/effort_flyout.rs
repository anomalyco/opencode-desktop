//! The lit model's efforts, beside its row in the model list (Desktop's flyout in
//! `select-dialog.tsx`): a press on the row's "Xhigh ›" opens it, and picking one picks the
//! model with that effort.

use rune::{Align, El, Side, Tx, col};

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
    /// Opens the model `key`'s efforts beside its row, or closes them when they're open.
    pub fn toggle_effort_flyout(&mut self, key: &str) {
        let open = self.pickers.flyout.as_ref().is_some_and(|f| f.model == key);
        self.pickers.flyout = (!open && self.model_effort(key).is_some()).then(|| Flyout { model: key.to_owned(), lit: None });
        self.pickers.activate(key.to_owned());
        self.pickers.search.focused = true;
    }

    /// Picks the model `key` with `effort` ("" for its default).
    pub fn choose_effort(&mut self, key: &str, effort: &str, tx: &mut Tx) {
        self.pickers.stage_effort(key, effort);
        self.pickers.flyout = None;
        self.choose_picker_tx(key, tx);
    }
}

/// The flyout, when one is open: Default and each effort, the one the model would run with
/// checked.
pub fn view(s: &App, p: &Pal) -> Option<El> {
    let f = s.pickers.flyout.as_ref().filter(|_| s.pickers.open == Some(Kind::Model))?;
    let model = s.pickers.models.iter().find(|m| m.key == f.model)?;
    let now = s.model_effort(&f.model)?;
    let mut list = col().key("effort-options").w_full().padding(MENU.inset);
    for effort in std::iter::once(String::new()).chain(model.variants.iter().cloned()) {
        let label = if effort.is_empty() { "Default".to_owned() } else { capitalize(&effort) };
        let id = if effort.is_empty() { "default".to_owned() } else { effort.clone() };
        let o = Opt::new(format!("effort-{id}"), label).checked(effort == now).active(f.lit.as_deref() == Some(id.as_str()));
        let (key, lit) = (f.model.clone(), id.clone());
        let row = pop::option(p, MENU, o, move |s: &mut App| {
            if let Some(f) = &mut s.pickers.flyout { f.lit = Some(lit.clone()); }
        }, move |s: &mut App, tx: &mut Tx| s.choose_effort(&key, &effort, tx));
        list = list.child(row);
    }
    let menu = pop::surface(p, MENU, "effort-flyout", "Choose effort").w(160.0).child(list);
    // Beside the row, clear of the list's edge.
    Some(menu.anchored_fit(format!("option-{}", f.model), Side::Right, Align::Start, pop::LIST.inset + 6.0))
}
