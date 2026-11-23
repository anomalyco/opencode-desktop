//! Key chords (`shell/commands/keymap.ts`, `command.tsx`, opencode#64395): a command's keybind
//! can be several strokes pressed one after another, written with spaces between them
//! ("mod+k mod+c"). A `rune::Keybind` holds single strokes only, so the chords live here: how a
//! saved keybind splits into them, the ones Settings › Shortcuts bound ([`all`]), the sequence
//! in progress ([`Pending`], one explicit machine) and how a chord reads on a label.

use std::cell::RefCell;

use rune::{Chord, ChordKey, ChordMod, KeyPlatform, Keybind};

/// A command chord takes at most this many strokes (`MAX_CHORD_STROKES`).
pub const MAX_STROKES: usize = 4;

/// One combo of a saved keybind (`chordStrokes`): its strokes, one for a plain shortcut. `None`
/// when a stroke doesn't parse.
pub fn strokes(combo: &str) -> Option<Vec<Chord>> {
    // Rune's own syntax allows spaces around "+" ("mod + k"): that is one stroke.
    if let Ok(chord) = combo.parse::<Chord>() {
        return Some(vec![chord]);
    }
    let strokes: Vec<Chord> = combo.split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
    (!strokes.is_empty()).then_some(strokes)
}

/// A saved keybind read as Desktop reads it: the single strokes as a `Keybind` (`parseKeybind`
/// leaves chords out) and the chords, each with whether it is the keybind's first combo, the
/// one labels show (`formatKeybind`).
pub struct Parsed {
    pub keybind: Keybind,
    pub chords: Vec<(Vec<Chord>, bool)>,
}

/// Reads `spec` ("mod+shift+l", "mod+k mod+c", "none"); `None` when a combo doesn't parse.
pub fn parse(spec: &str) -> Option<Parsed> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") {
        return Some(Parsed { keybind: Keybind::default(), chords: Vec::new() });
    }
    let mut singles = Vec::new();
    let mut chords = Vec::new();
    for (at, combo) in spec.split(',').enumerate() {
        let mut strokes = strokes(combo)?;
        if strokes.len() == 1 {
            singles.push(strokes.remove(0));
        } else {
            chords.push((strokes, at == 0));
        }
    }
    Some(Parsed { keybind: Keybind::from(singles.as_slice()), chords })
}

/// A chord as Settings saves it: each stroke's canonical form, a space between ("mod+k mod+c").
pub fn spec(strokes: &[Chord]) -> String {
    strokes.iter().map(Chord::to_string).collect::<Vec<_>>().join(" ")
}

/// How a chord reads (`formatKeybind`): its strokes' labels with a space between ("Ctrl+K Ctrl+C").
pub fn label(strokes: &[Chord]) -> String {
    strokes.iter().map(|c| super::label(*c)).collect::<Vec<_>>().join(" ")
}

/// A chord's keycaps (`formatKeybindParts`): one per stroke (⌘K, ⌘C), where a single stroke
/// gives one per key.
pub fn caps(strokes: &[Chord]) -> Vec<String> {
    match strokes {
        [one] => super::caps(*one),
        many => many.iter().map(|c| super::label(*c)).collect(),
    }
}

/// Whether two chords clash (`clashes`): one starts the other, so the shorter would run before
/// the longer could finish. Two equal chords clash too.
pub fn clashes(a: &[Chord], b: &[Chord]) -> bool {
    let platform = KeyPlatform::current();
    a.iter().zip(b).all(|(x, y)| x.same_keys(y, platform))
}

/// Whether a chord's first stroke types a character (`typesCharacter`): no Ctrl, Alt or logo
/// key, and not Tab. Such a chord stays out of text fields, so typing never starts one.
pub fn types_character(first: Chord) -> bool {
    let held = first.mods_on(KeyPlatform::current());
    !held.iter().any(|m| matches!(m, ChordMod::Primary | ChordMod::Ctrl | ChordMod::Alt | ChordMod::Super)) && first.key() != ChordKey::Tab
}

/// A chord a command answers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    /// The command's id.
    pub id: String,
    pub strokes: Vec<Chord>,
    /// The chord is its keybind's first combo, so the command's labels show it.
    pub first: bool,
}

/// Every chord Settings › Shortcuts bound (`overrides::apply`).
pub fn all() -> Vec<Bound> {
    super::overrides::chords()
}

/// Whether any chord is bound.
pub fn any() -> bool {
    super::overrides::has_chords()
}

/// The chord command `id`'s labels show: its keybind's first combo, when that is a chord.
pub fn first(id: &str) -> Option<Vec<Chord>> {
    super::overrides::first_chord(id)
}

/// The sequence in progress (`keymap.on("pendingSequence")`): nothing, or the strokes pressed so
/// far, each as the chord of the binding it matched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Pending {
    #[default]
    Idle,
    Held(Vec<Chord>),
}

impl Pending {
    pub fn strokes(&self) -> &[Chord] {
        match self {
            Self::Idle => &[],
            Self::Held(strokes) => strokes,
        }
    }
}

/// A key as the sequence sees it.
pub enum Stroke<'a> {
    /// Escape alone: clears the sequence (`registerEscapeClearsPendingSequence`).
    Escape,
    /// Backspace alone: drops its last stroke (`registerBackspacePopsPendingSequence`).
    Backspace,
    /// Any other key: `is` says whether it is a given chord, and `editing` whether a text
    /// field has the keyboard.
    Key { is: &'a dyn Fn(&Chord) -> bool, editing: bool },
    /// Shortcuts are off (Settings is recording one, a dialog is open): nothing is in progress.
    Blocked,
}

/// What a key did to the sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not a sequence's key: it goes on to whatever else takes it. A key that doesn't continue
    /// the sequence in progress ends it and goes on too (`sequence-miss`).
    Passed { ended: bool },
    /// Taken: the sequence started, grew, shrank or was cleared.
    Held,
    /// Taken: it finished command `id`'s chord.
    Run(String),
}

/// The one transition: `key` against the sequence in progress and the chords bound.
pub fn step(state: &mut Pending, bound: &[Bound], key: Stroke<'_>) -> Outcome {
    let platform = KeyPlatform::current();
    let held = state.strokes().to_vec();
    match key {
        Stroke::Blocked => Outcome::Passed { ended: std::mem::take(state) != Pending::Idle },
        Stroke::Escape | Stroke::Backspace if held.is_empty() => Outcome::Passed { ended: false },
        Stroke::Escape => {
            *state = Pending::Idle;
            Outcome::Held
        }
        Stroke::Backspace => {
            *state = if held.len() > 1 { Pending::Held(held[..held.len() - 1].to_vec()) } else { Pending::Idle };
            Outcome::Held
        }
        Stroke::Key { is, editing } => {
            // The chords the strokes so far start, that this key continues.
            let next: Vec<&Bound> = bound
                .iter()
                .filter(|b| b.strokes.len() > held.len() && held.iter().zip(&b.strokes).all(|(x, y)| x.same_keys(y, platform)))
                .filter(|b| !(held.is_empty() && editing && types_character(b.strokes[0])))
                .filter(|b| is(&b.strokes[held.len()]))
                .collect();
            let Some(first) = next.first() else {
                return Outcome::Passed { ended: std::mem::take(state) != Pending::Idle };
            };
            if let Some(done) = next.iter().find(|b| b.strokes.len() == held.len() + 1) {
                *state = Pending::Idle;
                return Outcome::Run(done.id.clone());
            }
            let mut strokes = held;
            strokes.push(first.strokes[strokes.len()]);
            *state = Pending::Held(strokes);
            Outcome::Held
        }
    }
}

/// A key that continues the sequence in progress, for its hint (`getActiveKeys`): the stroke,
/// and the command it runs, or `None` when more strokes follow.
pub fn continuations(state: &Pending, bound: &[Bound]) -> Vec<(Chord, Option<String>)> {
    let platform = KeyPlatform::current();
    let held = state.strokes();
    let mut out: Vec<(Chord, Option<String>)> = Vec::new();
    if held.is_empty() {
        return out;
    }
    for b in bound.iter().filter(|b| b.strokes.len() > held.len() && held.iter().zip(&b.strokes).all(|(x, y)| x.same_keys(y, platform))) {
        let stroke = b.strokes[held.len()];
        let ends = b.strokes.len() == held.len() + 1;
        match out.iter_mut().find(|(c, _)| c.same_keys(&stroke, platform)) {
            // A stroke that both runs a command and goes on shows as going on.
            Some((_, command)) if !ends => *command = None,
            Some(_) => {}
            None => out.push((stroke, ends.then(|| b.id.clone()))),
        }
    }
    out
}

thread_local! {
    /// The command whose chord just finished, while its key runs through the key handlers.
    static FIRED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Runs `f` with command `id` fired: every `Bind::matches` answers for `id` alone, whatever
/// key it is asked about, so the handler that owns the command runs it as its own shortcut would.
pub fn with_fired<R>(id: &str, f: impl FnOnce() -> R) -> R {
    let before = FIRED.with(|fired| fired.replace(Some(id.to_owned())));
    let out = f();
    FIRED.with(|fired| *fired.borrow_mut() = before);
    out
}

/// While a chord's command is fired ([`with_fired`]): whether it is `id`. `None` otherwise.
pub fn fired_is(id: &str) -> Option<bool> {
    FIRED.with(|fired| fired.borrow().as_deref().map(|f| f == id))
}
