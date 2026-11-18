//! The session view's own shortcuts, as data: a `rune::Keybind` per command with the chords each
//! platform uses (`mod` is ⌘ on macOS, Ctrl elsewhere). A host app lists these beside its own
//! (`Bind::ALL`) and draws their labels from here; never hard-code ⌘ ⌥ ⌃ ⇧.

use std::sync::LazyLock;

use rune::{Chord, ChordKey, KeyEvent, KeyPlatform, Keybind, Tx};

use KeyPlatform::{Linux, Mac, Windows};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bind {
    /// The question dock's previous question (`session-question-dock-classic.tsx:455`).
    QuestionBack,
    /// The question dock's Next or Submit.
    QuestionAdvance,
    /// Stops the running session from the composer (Esc does too).
    Stop,
    /// A popover's next and previous row, besides ↑ ↓.
    PopoverNext,
    PopoverPrev,
    /// The same with Alt held, where a popover allows it.
    PopoverNextAlt,
    PopoverPrevAlt,
    /// Stars or unstars the highlighted model.
    Favorite,
    /// The composer's attach button.
    Attach,
    /// Open project, from the project picker's add row.
    OpenProject,
    /// The composer's pickers: choose a model, cycle the agent (Shift: backwards), cycle the variant.
    Model,
    Agent,
    AgentBack,
    Variant,
    /// Zoom, the app's or an open diagram's.
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Bind {
    pub const ALL: [Bind; 17] = [
        Bind::QuestionBack,
        Bind::QuestionAdvance,
        Bind::Stop,
        Bind::PopoverNext,
        Bind::PopoverPrev,
        Bind::PopoverNextAlt,
        Bind::PopoverPrevAlt,
        Bind::Favorite,
        Bind::Attach,
        Bind::OpenProject,
        Bind::Model,
        Bind::Agent,
        Bind::AgentBack,
        Bind::Variant,
        Bind::ZoomIn,
        Bind::ZoomOut,
        Bind::ZoomReset,
    ];

    /// The chords bound here: the user's, when the host set some ([`set_override`]), else the
    /// default.
    pub fn keybind(self) -> &'static Keybind {
        OVERRIDES.read().unwrap_or_else(|e| e.into_inner())[self as usize].unwrap_or(&TABLE[self as usize])
    }

    /// The chords this command has before the user changes them.
    pub fn default_keybind(self) -> &'static Keybind {
        &TABLE[self as usize]
    }

    pub fn matches(self, ev: &KeyEvent) -> bool {
        self.keybind().matches(ev)
    }

    pub fn matches_tx(self, ev: &KeyEvent, tx: &Tx) -> bool {
        self.keybind().matches_tx(ev, tx)
    }

    /// How the first chord reads here ("⌘⏎", "Ctrl+⏎"); "" when unbound here.
    pub fn label(self) -> String {
        self.keybind().chords().first().map_or_else(String::new, |c| label(*c))
    }
}

/// The host's shortcut settings over the defaults, in `Bind` order.
static OVERRIDES: std::sync::RwLock<[Option<&'static Keybind>; 17]> = std::sync::RwLock::new([None; 17]);

/// Binds `bind` to `keybind` instead of its default, or back to the default with `None`: the
/// host's shortcut settings, which it keeps for the app's life.
pub fn set_override(bind: Bind, keybind: Option<&'static Keybind>) {
    OVERRIDES.write().unwrap_or_else(|e| e.into_inner())[bind as usize] = keybind;
}

// In `Bind` order.
static TABLE: LazyLock<[Keybind; 17]> = LazyLock::new(|| {
    [
        Keybind::new("mod+[").on(Windows, "alt+left").on(Linux, "alt+left"),
        // ⌃↩ also moves on, on macOS.
        Keybind::new("mod+enter, ctrl+enter, mod+shift+enter, ctrl+shift+enter"),
        Keybind::new("ctrl+g"),
        // Emacs-style rows on macOS only: off macOS Ctrl+N and Ctrl+P are the app's New session
        // and Open file, which Desktop's window-wide keymap takes before a popover sees them.
        Keybind::new("ctrl+n, ctrl+shift+n").on(Windows, "none").on(Linux, "none"),
        Keybind::new("ctrl+p, ctrl+shift+p").on(Windows, "none").on(Linux, "none"),
        Keybind::new("ctrl+alt+n, ctrl+alt+shift+n").on(Windows, "none").on(Linux, "none"),
        Keybind::new("ctrl+alt+p, ctrl+alt+shift+p").on(Windows, "none").on(Linux, "none"),
        Keybind::new("mod+f"),
        Keybind::new("mod+u"),
        Keybind::new("mod+o"),
        Keybind::new("mod+'"),
        Keybind::new("mod+."),
        Keybind::new("mod+shift+."),
        Keybind::new("mod+shift+d"),
        // ⌘= is the unshifted key; ⌘+ is the same key with Shift, or the keypad's.
        Keybind::new("mod+=, mod+plus, mod+shift+="),
        Keybind::new("mod+-, mod+shift+-"),
        Keybind::new("mod+0"),
    ]
});

/// A chord's label here: "⇧⌘T" on macOS, "Ctrl+Shift+T" elsewhere (`Chord::label`), with Enter
/// as ⏎ on every platform, as Desktop's submit hints draw it ("⌘⏎", "Ctrl+⏎").
pub fn label(chord: Chord) -> String {
    caps(chord).join(if KeyPlatform::current() == Mac { "" } else { "+" })
}

/// A chord's keycaps here (`Chord::keycaps_on`), with Enter as ⏎ and, off macOS, the words in
/// the current language (`common.key.*`: Strg, Umschalt).
pub fn caps(chord: Chord) -> Vec<String> {
    let mut caps: Vec<String> = chord.keycaps_on(KeyPlatform::current()).into_iter().map(|cap| localized(&cap).map_or(cap, str::to_owned)).collect();
    if chord.key() == ChordKey::Enter
        && let Some(last) = caps.last_mut()
    {
        *last = "⏎".into();
    }
    caps
}

/// A chord as Desktop's `formatKeybind` writes it in a sentence: the keys' words in the current
/// language, Enter as a word ("Strg+Eingabe"), joined with "+" off macOS.
pub fn sentence(chord: Chord) -> String {
    let platform = KeyPlatform::current();
    let caps: Vec<String> = chord.keycaps_on(platform).into_iter().map(|cap| localized(&cap).map_or(cap, str::to_owned)).collect();
    caps.join(if platform == Mac { "" } else { "+" })
}

/// A keycap word Desktop translates (`displayKeybindParts`), in the current language.
fn localized(cap: &str) -> Option<&'static str> {
    use opencode_i18n::t;
    Some(match cap {
        "Ctrl" => t!("common.key.ctrl"),
        "Alt" => t!("common.key.alt"),
        "Shift" => t!("common.key.shift"),
        "Enter" => t!("common.key.enter"),
        "Tab" => t!("common.key.tab"),
        "Esc" => t!("common.key.esc"),
        "Backspace" => t!("common.key.backspace"),
        "Delete" => t!("common.key.delete"),
        "Insert" => t!("common.key.insert"),
        "Home" => t!("common.key.home"),
        "End" => t!("common.key.end"),
        "Page Up" => t!("common.key.pageUp"),
        "Page Down" => t!("common.key.pageDown"),
        "Space" => t!("common.key.space"),
        _ => return None,
    })
}
