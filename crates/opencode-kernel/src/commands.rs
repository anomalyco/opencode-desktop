//! Unified command registry shared across command palettes, slash menus, and shortcut sheets.

use std::collections::HashMap;
use std::fmt;

/// Localized or static label for a command title or description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandTitle {
    /// Compile-time validated key resolved through `opencode-i18n`.
    Key(opencode_i18n::Key),
    /// Static literal for apps or dynamic features without a catalog key.
    Static(&'static str),
}

impl CommandTitle {
    /// Resolves this title in the active locale.
    pub fn resolve(self) -> &'static str {
        match self {
            Self::Key(key) => opencode_i18n::I18N.text(key),
            Self::Static(text) => text,
        }
    }
}

impl From<opencode_i18n::Key> for CommandTitle {
    fn from(key: opencode_i18n::Key) -> Self {
        Self::Key(key)
    }
}

impl From<&'static str> for CommandTitle {
    fn from(text: &'static str) -> Self {
        Self::Static(text)
    }
}

/// One registered command entry targeting host state `S`.
pub struct CommandEntry<S> {
    pub id: &'static str,
    pub title: CommandTitle,
    pub description: Option<CommandTitle>,
    pub slash: Option<&'static str>,
    pub keybind: Option<&'static str>,
    /// Optional host keymap binding id (for example `"settings.open"` or `"mcp.toggle"`).
    pub bind_id: Option<&'static str>,
    pub section: Option<&'static str>,
    pub hidden: bool,
    /// Ascending palette display order (ties preserve registration order; defaults to `u16::MAX`).
    pub order: u16,
    /// Dynamic title resolver evaluated against live host state, overriding [`Self::title`].
    pub titled: Option<fn(&S) -> CommandTitle>,
    pub enabled: fn(&S) -> bool,
    pub run: fn(&mut S, &mut rune::Tx),
}

impl<S> Clone for CommandEntry<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S> Copy for CommandEntry<S> {}

impl<S> fmt::Debug for CommandEntry<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandEntry")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("slash", &self.slash)
            .field("keybind", &self.keybind)
            .field("bind_id", &self.bind_id)
            .field("section", &self.section)
            .field("hidden", &self.hidden)
            .field("order", &self.order)
            .finish()
    }
}

fn always_enabled<S>(_: &S) -> bool {
    true
}

impl<S> CommandEntry<S> {
    /// Creates an enabled command with a localized or static title.
    pub fn new(id: &'static str, title: impl Into<CommandTitle>, run: fn(&mut S, &mut rune::Tx)) -> Self {
        Self {
            id,
            title: title.into(),
            description: None,
            slash: None,
            keybind: None,
            bind_id: None,
            section: None,
            hidden: false,
            order: u16::MAX,
            titled: None,
            enabled: always_enabled,
            run,
        }
    }

    /// Const constructor backed by an `opencode-i18n` key.
    pub const fn from_key(id: &'static str, title: opencode_i18n::Key, run: fn(&mut S, &mut rune::Tx)) -> Self {
        Self {
            id,
            title: CommandTitle::Key(title),
            description: None,
            slash: None,
            keybind: None,
            bind_id: None,
            section: None,
            hidden: false,
            order: u16::MAX,
            titled: None,
            enabled: always_enabled,
            run,
        }
    }

    /// Const constructor backed by a static string.
    pub const fn from_static(id: &'static str, title: &'static str, run: fn(&mut S, &mut rune::Tx)) -> Self {
        Self {
            id,
            title: CommandTitle::Static(title),
            description: None,
            slash: None,
            keybind: None,
            bind_id: None,
            section: None,
            hidden: false,
            order: u16::MAX,
            titled: None,
            enabled: always_enabled,
            run,
        }
    }

    /// Attaches a description shown below the command title.
    pub fn describe(mut self, description: impl Into<CommandTitle>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attaches a composer `/slash` trigger name (without a leading `/`).
    pub const fn slash(mut self, name: &'static str) -> Self {
        self.slash = Some(name);
        self
    }

    /// Attaches a default shortcut spec (for example `"mod+shift+k"`), overridable by command id.
    pub const fn keybind(mut self, spec: &'static str) -> Self {
        self.keybind = Some(spec);
        self
    }

    /// Links the command to a host keymap binding id (for example `"settings.open"`).
    pub const fn bind_id(mut self, id: &'static str) -> Self {
        self.bind_id = Some(id);
        self
    }

    /// Assigns the command to a named palette/settings section.
    pub const fn section(mut self, section: &'static str) -> Self {
        self.section = Some(section);
        self
    }

    /// Marks the command as hidden from default palette listings while staying invokable.
    pub const fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Sets the command's ascending palette display order (defaults to `u16::MAX`).
    pub const fn order(mut self, order: u16) -> Self {
        self.order = order;
        self
    }

    /// Attaches a dynamic title resolver evaluated against live host state, overriding [`Self::title`].
    pub const fn titled(mut self, titled: fn(&S) -> CommandTitle) -> Self {
        self.titled = Some(titled);
        self
    }

    /// Restricts when the command is listed and executable.
    pub const fn when(mut self, enabled: fn(&S) -> bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Resolves the command's static title in the active locale.
    pub fn title_text(&self) -> &'static str {
        self.title.resolve()
    }

    /// Resolves the command's title for `state` in the active locale.
    pub fn title_for(&self, state: &S) -> &'static str {
        self.titled.map_or(self.title, |f| f(state)).resolve()
    }

    /// Resolves the command's description in the active locale, if present.
    pub fn description_text(&self) -> Option<&'static str> {
        self.description.map(CommandTitle::resolve)
    }

    /// Parses the default [`Self::keybind`] spec into a non-empty [`rune::Keybind`], if set.
    pub fn parsed_keybind(&self) -> Option<rune::Keybind> {
        self.keybind
            .and_then(|spec| spec.parse::<rune::Keybind>().ok())
            .filter(|kb| !kb.chords().is_empty())
    }

    /// First default [`rune::Chord`] parsed from [`Self::keybind`], if any.
    pub fn chord(&self) -> Option<rune::Chord> {
        self.parsed_keybind().and_then(|kb| kb.chords().first().copied())
    }

    /// Evaluates whether this command is currently enabled for `state`.
    pub fn is_enabled(&self, state: &S) -> bool {
        (self.enabled)(state)
    }
}

/// Ordered registry of [`CommandEntry`] definitions indexed by id and slash trigger.
pub struct CommandRegistry<S> {
    entries: Vec<CommandEntry<S>>,
    keybinds: Vec<Option<rune::Keybind>>,
    by_id: HashMap<&'static str, usize>,
    by_slash: HashMap<&'static str, usize>,
}

impl<S> Default for CommandRegistry<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> CommandRegistry<S> {
    /// Creates an empty command registry.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            keybinds: Vec::new(),
            by_id: HashMap::new(),
            by_slash: HashMap::new(),
        }
    }

    /// Registers a command, replacing any existing entry with the same `id` in place.
    pub fn register(&mut self, entry: CommandEntry<S>) {
        let parsed = entry.parsed_keybind();
        if let Some(&idx) = self.by_id.get(entry.id) {
            if let Some(old_slash) = self.entries[idx].slash {
                self.by_slash.remove(old_slash);
            }
            if let Some(slash) = entry.slash {
                self.by_slash.insert(slash, idx);
            }
            self.entries[idx] = entry;
            self.keybinds[idx] = parsed;
            return;
        }
        let idx = self.entries.len();
        self.by_id.insert(entry.id, idx);
        if let Some(slash) = entry.slash {
            self.by_slash.insert(slash, idx);
        }
        self.entries.push(entry);
        self.keybinds.push(parsed);
    }

    /// Removes a command by id, returning it if it was registered.
    pub fn unregister(&mut self, id: &str) -> Option<CommandEntry<S>> {
        let idx = self.by_id.remove(id)?;
        let removed = self.entries.remove(idx);
        self.keybinds.remove(idx);
        self.rebuild_indices();
        Some(removed)
    }

    /// Looks up a command by its unique id.
    pub fn get(&self, id: &str) -> Option<&CommandEntry<S>> {
        let &idx = self.by_id.get(id)?;
        self.entries.get(idx)
    }

    /// Looks up a command by its slash trigger (accepts `"foo"` or `"/foo"`).
    pub fn by_slash(&self, slash: &str) -> Option<&CommandEntry<S>> {
        let key = slash.strip_prefix('/').unwrap_or(slash);
        let &idx = self.by_slash.get(key)?;
        self.entries.get(idx)
    }

    /// Executes the command with `id` if it exists and is enabled for `state`.
    pub fn execute(&self, id: &str, state: &mut S, tx: &mut rune::Tx) -> bool {
        let Some(entry) = self.get(id).copied() else {
            return false;
        };
        if !(entry.enabled)(state) {
            return false;
        }
        (entry.run)(state, tx);
        true
    }

    /// Executes the command matching `slash` if it exists and is enabled for `state`.
    pub fn execute_slash(&self, slash: &str, state: &mut S, tx: &mut rune::Tx) -> bool {
        let Some(entry) = self.by_slash(slash).copied() else {
            return false;
        };
        if !(entry.enabled)(state) {
            return false;
        }
        (entry.run)(state, tx);
        true
    }

    /// Iterates all registered commands in registration order.
    pub fn iter(&self) -> std::slice::Iter<'_, CommandEntry<S>> {
        self.entries.iter()
    }

    /// Iterates registered commands together with their cached default first chord.
    pub fn iter_with_chords(&self) -> impl Iterator<Item = (&CommandEntry<S>, Option<rune::Chord>)> {
        self.entries
            .iter()
            .zip(self.keybinds.iter())
            .map(|(entry, kb)| (entry, kb.as_ref().and_then(|k| k.chords().first().copied())))
    }

    /// Finds the first enabled command associated with host keymap `bind_id`.
    pub fn find_bind(&self, bind_id: &str, state: &S) -> Option<CommandEntry<S>> {
        self.entries
            .iter()
            .find(|entry| entry.bind_id == Some(bind_id) && entry.is_enabled(state))
            .copied()
    }

    /// Finds the first enabled command matching `ev` against its default keybind.
    pub fn find_key(&self, state: &S, ev: &rune::KeyEvent, tx: &rune::Tx) -> Option<CommandEntry<S>> {
        self.find_key_with(state, ev, tx, |_| None)
    }

    /// Finds the first enabled command matching `ev`, letting the host supply keybind overrides
    /// (returning `Some(override_kb)` to replace or unbind the command's default keybind).
    /// Commands linked to a host keymap binding via [`CommandEntry::bind_id`] are skipped here.
    pub fn find_key_with<'a>(
        &'a self,
        state: &S,
        ev: &rune::KeyEvent,
        tx: &rune::Tx,
        override_for: impl Fn(&str) -> Option<&'a rune::Keybind>,
    ) -> Option<CommandEntry<S>> {
        let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo);
        if plain && tx.focused().is_some() {
            return None;
        }
        self.entries
            .iter()
            .zip(self.keybinds.iter())
            .find(|(entry, default_kb)| {
                if entry.bind_id.is_some() || !entry.is_enabled(state) {
                    return false;
                }
                let kb = match override_for(entry.id) {
                    Some(over) => Some(over),
                    None => default_kb.as_ref(),
                };
                kb.is_some_and(|kb| kb.matches_tx(ev, tx))
            })
            .map(|(entry, _)| *entry)
    }

    /// Iterates registered commands currently enabled for `state`.
    pub fn enabled<'a>(&'a self, state: &'a S) -> impl Iterator<Item = &'a CommandEntry<S>> {
        self.entries.iter().filter(move |entry| (entry.enabled)(state))
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no commands are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn rebuild_indices(&mut self) {
        self.by_id.clear();
        self.by_slash.clear();
        for (idx, entry) in self.entries.iter().enumerate() {
            self.by_id.insert(entry.id, idx);
            if let Some(slash) = entry.slash {
                self.by_slash.insert(slash, idx);
            }
        }
    }
}

impl<S> fmt::Debug for CommandRegistry<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandRegistry").field("len", &self.entries.len()).finish()
    }
}
