//! The text fields plugins draw (`ctx:CreateEditor`, `ui.field`): owned by the window's host, so
//! typing, IME, selection and undo stay native and a hot reload keeps them. A plugin's n-th
//! editor is the same field from one setup to the next.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use rune::TextEdit;

use super::cx;
use crate::App;

/// Every plugin's fields in one window, by `(plugin, index)`.
pub type Editors = RefCell<HashMap<(String, usize), TextEdit>>;

fn with<R>(f: impl FnOnce(&Editors) -> R) -> Option<R> {
    cx::app().map(|app| f(&app.plugins.editors))
}

/// Makes field `index` of `plugin` if this window has none yet.
pub fn ensure(plugin: &str, index: usize, initial: &str) {
    with(|e| e.borrow_mut().entry((plugin.to_owned(), index)).or_insert_with(|| TextEdit::new(initial)).text.len());
}

pub fn text(plugin: &str, index: usize) -> String {
    with(|e| e.borrow().get(&(plugin.to_owned(), index)).map(|t| t.text.clone())).flatten().unwrap_or_default()
}

/// Changes with the field's text, selection, composition and focus.
pub fn version(plugin: &str, index: usize) -> f64 {
    with(|e| version_of(e.borrow().get(&(plugin.to_owned(), index)))).unwrap_or(0) as f64
}

pub fn version_in(app: &App, plugin: &str, index: usize) -> u64 {
    version_of(app.plugins.editors.borrow().get(&(plugin.to_owned(), index)))
}

fn version_of(edit: Option<&TextEdit>) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    edit.hash(&mut h);
    h.finish() >> 11
}

pub fn set_text(plugin: &str, index: usize, text: &str) {
    with(|e| {
        if let Some(edit) = e.borrow_mut().get_mut(&(plugin.to_owned(), index)) {
            edit.replace_text(text);
        }
    });
    super::bump();
}

pub fn focus(plugin: &str, index: usize) {
    with(|e| {
        if let Some(edit) = e.borrow_mut().get_mut(&(plugin.to_owned(), index)) {
            edit.focused = true;
        }
    });
}

pub fn blur(plugin: &str, index: usize) {
    with(|e| {
        if let Some(edit) = e.borrow_mut().get_mut(&(plugin.to_owned(), index)) {
            edit.focused = false;
        }
    });
}

pub fn select_all(plugin: &str, index: usize) {
    with(|e| {
        if let Some(edit) = e.borrow_mut().get_mut(&(plugin.to_owned(), index)) {
            let len = edit.text.len();
            edit.set_selection(0, len);
        }
    });
}

pub fn focused(plugin: &str, index: usize) -> bool {
    with(|e| e.borrow().get(&(plugin.to_owned(), index)).is_some_and(|t| t.focused)).unwrap_or(false)
}

/// Whether field `index` of `plugin` has the keyboard.
pub fn focused_in(app: &App, plugin: &str, index: usize) -> bool {
    app.plugins.editors.borrow().get(&(plugin.to_owned(), index)).is_some_and(|t| t.focused)
}

/// Whether any plugin field in this window has the keyboard.
pub fn any_focused(app: &App) -> bool {
    app.plugins.editors.borrow().values().any(|t| t.focused)
}

/// Whether one of `plugin`'s fields has the keyboard.
pub fn plugin_focused(app: &App, plugin: &str) -> bool {
    app.plugins.editors.borrow().iter().any(|((p, _), t)| p == plugin && t.focused)
}

/// What an edit of field `index` of `plugin` changes.
pub fn get_mut<'a>(app: &'a mut App, plugin: &str, index: usize) -> &'a mut TextEdit {
    app.plugins.editors.get_mut().entry((plugin.to_owned(), index)).or_insert_with(|| TextEdit::new(""))
}

pub fn text_in(app: &App, plugin: &str, index: usize) -> String {
    app.plugins.editors.borrow().get(&(plugin.to_owned(), index)).map(|t| t.text.clone()).unwrap_or_default()
}

/// A plugin's field took an edit (a press focuses it, keys type in it): when its plugin's tab is the one a
/// side pane shows, the side region has the keyboard, as Desktop's side focus follows a focus or
/// click inside `#review-panel`.
pub fn note_side_field(app: &mut App, plugin: &str) {
    app.side.quiet = false;
    if app.side.focused && !app.composer.focused {
        return;
    }
    let shown = crate::panels::SESSION_PANES.into_iter().find(|&pane| {
        app.pane_shown_tab(pane).and_then(|key| app.resolve_side_region()?.entry(&key).map(|e| e.plugin == plugin)).unwrap_or(false)
    });
    if let Some(pane) = shown {
        app.side.focused = true;
        app.side.active_pane_focus = pane;
    }
}
