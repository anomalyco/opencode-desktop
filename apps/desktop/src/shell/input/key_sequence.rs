//! Key chords in progress (`shell/commands/command.tsx`, `keymap.ts`, `key-sequence.tsx`,
//! opencode#64395): a shortcut of several strokes ("Ctrl+K Ctrl+C") holds the keys it has so far,
//! and a card at the window's foot lists the keys that continue it and what each runs. Escape
//! clears it, Backspace drops its last stroke, and a key that doesn't continue it ends it and
//! goes on as any key. Desktop reads these keys on the document before the focused element:
//! here a focused field asks first ([`field_key`], from `App::shortcut`) and the window's key
//! handler asks before anything else ([`window_key`]).

use opencode_kernel::OverlayKind;
use opencode_ui::keymap::sequence::{self, Outcome, Pending, Stroke};
use opencode_ui::palette::{Elevate, Pal};
use rune::{Chord, El, Key, KeyEvent, Modifier, Mods, TextStyle, Tx, col, row, text};

use crate::App;
use crate::keymap::{self, Bind};

/// What the focused field's handler did with the last key it showed the chords: nothing to
/// remember, or it passed that key on, so the window's handler doesn't step the sequence again.
#[derive(Default, PartialEq)]
enum Seen {
    #[default]
    Nothing,
    Passed(KeyEvent),
}

#[derive(Default)]
pub(crate) struct State {
    pending: Pending,
    seen: Seen,
}

pub(crate) static SEQUENCE: opencode_kernel::Slot<State> = opencode_kernel::Slot::new();

/// `key-sequence.tsx`'s row text: 13px on the compact 16px line.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);

/// A key from a focused text field, before the field edits with it. A chord whose first key
/// types a character never starts here (`outsideText`).
pub(crate) fn field_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if resting(s) {
        return false;
    }
    let took = key(s, ev, true, tx);
    s.kernel.slots.get_mut(&SEQUENCE).seen = if took { Seen::Nothing } else { Seen::Passed(ev.clone()) };
    took
}

/// A key at the window's handler: nothing has focus, or what has passed the key on.
pub(crate) fn window_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if resting(s) {
        return false;
    }
    // The field that passed this key on already showed it to the sequence.
    if tx.focused().is_some() && s.kernel.slots.untracked(&SEQUENCE).seen == Seen::Passed(ev.clone()) {
        s.kernel.slots.get_mut(&SEQUENCE).seen = Seen::Nothing;
        return false;
    }
    key(s, ev, false, tx)
}

/// No chord is bound and none is in progress: keys cost nothing here.
fn resting(s: &App) -> bool {
    s.kernel.slots.untracked(&SEQUENCE).pending == Pending::Idle && !sequence::any()
}

fn key(s: &mut App, ev: &KeyEvent, editing: bool, tx: &mut Tx) -> bool {
    // A modifier going down is no stroke, and a fired command's stand-in key is none either.
    // Desktop's keymap takes the modifier's own keydown for a key that continues nothing, so there
    // Ctrl+K Ctrl+C runs only with Ctrl held from the first key to the last. That is a bug there
    // (opencode#65734, draft fix opencode#65735), not copied: letting the modifier go is fine here.
    if matches!(ev.key, Key::Modifier { .. }) {
        return false;
    }
    let bound = sequence::all();
    let bare = ev.mods == Mods::default();
    // `createAppKeymap`'s `blocked`: no shortcut runs while Settings records one or a dialog is open.
    let recording = s.settings_open() && s.kernel.slots.untracked(&crate::settings::SETTINGS_PAGES).shortcuts.capture.recording().is_some();
    let blocked = recording || s.kernel.overlays.has_kind(OverlayKind::Dialog);
    let mut pending = s.kernel.slots.untracked(&SEQUENCE).pending.clone();
    let outcome = {
        let is = |chord: &Chord| chord.matches_tx(ev, tx);
        let stroke = match ev.key {
            _ if blocked => Stroke::Blocked,
            Key::Escape if bare => Stroke::Escape,
            Key::Backspace if bare => Stroke::Backspace,
            _ => Stroke::Key { is: &is, editing },
        };
        sequence::step(&mut pending, &bound, stroke)
    };
    if s.kernel.slots.untracked(&SEQUENCE).pending != pending {
        tx.instant();
        s.kernel.slots.get_mut(&SEQUENCE).pending = pending;
    }
    match outcome {
        Outcome::Passed { .. } => false,
        Outcome::Held => true,
        Outcome::Run(id) => {
            run(s, &id, tx);
            true
        }
    }
}

/// Runs the command whose chord just finished. A table row's command lives in the key handler
/// that owns its shortcut, so the handlers run once more with that row fired: each
/// `Bind::matches` then answers for it alone, and the stand-in key (a modifier's release) is
/// nobody's shortcut. Any other command runs by its id, as the palette runs it.
fn run(s: &mut App, id: &str, tx: &mut Tx) {
    if Bind::by_id(id).is_none() {
        s.run_command(id, tx);
        return;
    }
    let stand_in = KeyEvent { key: Key::Modifier { modifier: Modifier::Fn, pressed: false }, mods: Mods::default() };
    sequence::with_fired(id, || super::keys::on_key(s, &stand_in, tx));
}

/// The command's title for the hint (`command.title(id)`), its id when it has none here.
fn title(s: &App, id: &str) -> String {
    if let Some(bind) = Bind::by_id(id) {
        return bind.binding().title();
    }
    s.catalog().into_iter().find(|c| c.id == id).map_or_else(|| id.to_owned(), |c| c.title.to_owned())
}

/// `KeySequenceHint`: the strokes so far as neutral keycaps, then a row per key that continues
/// them: its keycaps and the command it runs, or "…" when more keys follow. It takes no pointer.
pub(crate) fn view(s: &App, p: &Pal) -> Option<El> {
    let state = s.kernel.slots.get(&SEQUENCE);
    let held = state.pending.strokes();
    if held.is_empty() {
        return None;
    }
    let ghost = |chord: Chord| {
        keymap::caps(chord).into_iter().fold(row().dir(rune::Direction::Ltr).gap(2.0).center_y(), |r, cap| {
            r.child(row().h(14.0).min_w(14.0).px(4.0).center().child(text(cap.to_uppercase()).font(keymap::LABEL).color(p.faint)))
        })
    };
    let next = sequence::continuations(&state.pending, &sequence::all());
    let mut keys = col().key("key-sequence-keys").gap(4.0);
    let mut names = col().key("key-sequence-names").gap(4.0).grow(1.0).shrink().min_w(0.0);
    for (n, (chord, command)) in next.into_iter().enumerate() {
        let name = command.map_or_else(|| "…".to_owned(), |id| title(s, &id));
        keys = keys.child(row().key(format!("key-{n}")).h(16.0).center_y().child(ghost(chord)));
        names = names.child(text(name).key(format!("name-{n}")).font(ROW).color(p.text).truncate());
    }
    let card = col()
        .key("key-sequence-hint")
        .role("status")
        .min_w(240.0)
        .padding(12.0)
        .gap(8.0)
        // ounded-xl is 10px in Desktop's theme (measured on the card: border-radius 10px).
        .radius(10.0)
        .fill(p.bg_base)
        .elevate(p.floating)
        .child(keymap::chips(p, held.iter().flat_map(|c| keymap::caps(*c)).collect()))
        .child(row().gap(12.0).child(keys).child(names));
    Some(row().key("key-sequence").pin(Some(0.0), None, Some(0.0), Some(24.0)).justify_center().inert().raise().child(card))
}
