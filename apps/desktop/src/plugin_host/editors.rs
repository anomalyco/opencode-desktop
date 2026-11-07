//! `PluginEditors`: the text fields plugins draw, owned by the host so typing, IME, selection
//! and undo stay native, and a hot reload keeps them.

use std::hash::{Hash, Hasher};

use opencode_plugin::EditorHandle;
use rune::TextEdit;

pub struct PluginEditors {
    slots: Vec<Option<TextEdit>>,
    free: Vec<u32>,
    /// What a stale handle edits, so a field never panics on a reload race.
    spare: TextEdit,
}

impl Default for PluginEditors {
    fn default() -> Self {
        PluginEditors { slots: Vec::new(), free: Vec::new(), spare: TextEdit::new("") }
    }
}

impl PluginEditors {
    pub fn create(&mut self, initial: &str) -> EditorHandle {
        let edit = Some(TextEdit::new(initial));
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = edit;
            return EditorHandle(i);
        }
        self.slots.push(edit);
        EditorHandle(self.slots.len() as u32 - 1)
    }

    pub fn get(&self, h: EditorHandle) -> Option<&TextEdit> {
        self.slots.get(h.0 as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, h: EditorHandle) -> &mut TextEdit {
        match self.slots.get_mut(h.0 as usize).and_then(Option::as_mut) {
            Some(edit) => edit,
            None => &mut self.spare,
        }
    }

    pub fn remove(&mut self, h: EditorHandle) {
        if let Some(slot) = self.slots.get_mut(h.0 as usize)
            && slot.take().is_some()
        {
            self.free.push(h.0);
        }
    }

    /// Changes with the field's text, selection, composition and focus.
    pub fn version(&self, h: EditorHandle) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        self.get(h).hash(&mut hasher);
        hasher.finish()
    }
}
