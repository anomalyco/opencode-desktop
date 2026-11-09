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
    cx::app().map(|app| f(&app.plugins.luau.editors))
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
    version_of(app.plugins.luau.editors.borrow().get(&(plugin.to_owned(), index)))
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

/// What an edit of field `index` of `plugin` changes.
pub fn get_mut<'a>(app: &'a mut App, plugin: &str, index: usize) -> &'a mut TextEdit {
    app.plugins.luau.editors.get_mut().entry((plugin.to_owned(), index)).or_insert_with(|| TextEdit::new(""))
}

pub fn text_in(app: &App, plugin: &str, index: usize) -> String {
    app.plugins.luau.editors.borrow().get(&(plugin.to_owned(), index)).map(|t| t.text.clone()).unwrap_or_default()
}
