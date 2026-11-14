//! Settings › Shortcuts (`settings/keybinds/keybinds.tsx`, `keybinds/machine.ts`): every command
//! by group (General, Session, Navigation, Model and agent, Terminal, Prompt), each with its chord
//! or Unassigned. Pressing one records the next chord: Escape stops, Backspace or Delete alone
//! unbinds, a chord another command has is refused with a toast naming it, any other becomes the
//! command's. Unbind clears it while recording; Reset to defaults clears every change. The
//! changes are saved (`keybinds`) and every key handler takes them at once (`keymap::overrides`).
//! Typing anywhere on the page searches it.

use rune::{Chord, ChordKey, ChordMod, El, Key, KeyEvent, KeyPlatform, Keybind, TextEdit, TextStyle, Tx, col, row, text};

use super::{filter_field, ghost_button, header_beside, no_results};
use crate::App;
use crate::keymap::{Section, all, overrides};
use crate::palette::Pal;
use opencode_i18n::t;

/// A command's title and the keys' label: 13 on 16, -0.04 tracking.
const TITLE: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const KEYS: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// A shortcut's button (a `button-v2`) sets figures at one width (`button.css:21`).
const KEYS_BUTTON: TextStyle = KEYS.features("\"tnum\"");
const UNASSIGNED: TextStyle = TITLE.features("\"tnum\"");

#[derive(Default)]
pub struct State {
    pub filter: TextEdit,
    /// The command recording a chord, by id.
    pub capture: Option<String>,
}

/// One listed command.
struct Entry {
    id: String,
    title: String,
    section: Section,
    /// What it's bound to here now.
    keybind: Keybind,
}

/// `groupFor`: a command without a section of its own goes by its id's prefix.
fn group_for(id: &str) -> Section {
    let starts = |p: &[&str]| p.iter().any(|x| id.starts_with(x));
    if starts(&["model.", "agent.", "mcp."]) {
        Section::Models
    } else if starts(&["file.", "fileTree."]) {
        Section::Navigation
    } else if starts(&["prompt."]) {
        Section::Prompt
    } else if starts(&["terminal."]) {
        Section::Terminal
    } else if starts(&["session.", "message.", "permissions.", "steps.", "review."]) {
        Section::Session
    } else {
        Section::General
    }
}

/// The table's listed rows, then every command without one, offered here or not (`listFor` over
/// `command.catalog`), Unassigned unless bound. A command with a row goes by its row, so an
/// unlisted row (zoom) keeps its command out too; two commands may share a title, as Desktop's
/// New session and Open file do.
fn entries(s: &App) -> Vec<Entry> {
    let mut out: Vec<Entry> = all().iter().filter(|b| b.listed).map(|b| Entry { id: b.id.to_owned(), title: b.title(), section: b.section, keybind: b.bind.keybind().clone() }).collect();
    for c in s.catalog() {
        if c.keys.is_some() || out.iter().any(|e| e.id == c.id) {
            continue;
        }
        let keybind = overrides::command(c.id).cloned().or_else(|| c.chord.map(Keybind::from)).unwrap_or_default();
        let section = c.section.or_else(|| crate::commands::section_of(c.id)).unwrap_or_else(|| group_for(c.id));
        out.push(Entry { id: c.id.to_owned(), title: c.title.to_owned(), section, keybind });
    }
    out
}

fn label(k: &Keybind) -> String {
    k.chords().first().map_or_else(String::new, |c| crate::keymap::label(*c))
}

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings_pages.shortcuts;
    let query = st.filter.text.trim().to_owned();
    let list = entries(s);
    let changed = !s.settings.keybinds.is_empty();
    let reset = ghost_button(p, "settings-shortcuts-reset", t!("settings.shortcuts.reset.button"), None, false, changed, |s, _| reset(s));
    let head = col().w_full().gap(24.0)
        .child(header_beside(p, t!("settings.shortcuts.title"), t!("settings.shortcuts.description"), reset))
        .child(filter_field(s, p, "settings-shortcuts-search", t!("settings.shortcuts.search.placeholder"), &st.filter, |s| &mut s.settings_pages.shortcuts.filter));
    let mut body = col().key("settings-shortcuts").w_full().gap(32.0);
    let mut any = false;
    for section in Section::ALL {
        let mut rows: Vec<&Entry> = list.iter().filter(|e| e.section == section)
            .filter(|e| query.is_empty() || session_view::pickers::search::score(&query, &[&e.title, &label(&e.keybind)]).is_some())
            .collect();
        if rows.is_empty() {
            continue;
        }
        any = true;
        // By title (`localeCompare`); two of one title in Desktop's catalog order, which their ids keep.
        rows.sort_by(|a, b| rune::text::compare_names(&a.title, &b.title).then_with(|| a.id.cmp(&b.id)));
        let n = rows.len();
        let mut card = col().key(format!("settings-shortcuts-{}", section.title())).w_full().padding(20.0).radius(6.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted));
        for (i, e) in rows.into_iter().enumerate() {
            let target = format!("shortcut-{}", e.id);
            // 16 below, then Desktop's `border-b` (`py-3` the settings list sets to 16 below),
            // taking its device pixel of the row, then 16 to the next.
            let ruled = i + 1 < n;
            let r = row().key(format!("setting-{target}")).w_full().gap(16.0).center_y().pb(if ruled { 16.0 } else { 0.0 }).mb(if ruled { 16.0 } else { 0.0 })
                .border_sides([0.0, 0.0, if ruled { 0.5 } else { 0.0 }, 0.0], p.border)
                .border_layout()
                .child(text(e.title.clone()).font(TITLE).color(p.text).grow(1.0).shrink().min_w(0.0).truncate())
                .child(controls(s, p, e));
            card = card.child(super::revealable(s, p, &target, 20.0, r));
        }
        body = body.child(super::section(p, &format!("settings-shortcuts-section-{}", section.title()), Some(section.title()), card));
    }
    if !any && !query.is_empty() {
        body = body.child(no_results(p, "settings-shortcuts-empty", &query));
    }
    super::page(p, "settings-shortcuts-page", head, body)
}

/// Unbind while recording, then the chord's button: its keys, Unassigned, or Press keys.
fn controls(s: &App, p: &Pal, e: &Entry) -> El {
    let recording = s.settings_pages.shortcuts.capture.as_deref() == Some(e.id.as_str());
    let keys = label(&e.keybind);
    let (words, ink, font) = if recording {
        (t!("settings.shortcuts.pressKeys").to_owned(), p.text, KEYS_BUTTON)
    } else if keys.is_empty() {
        (t!("settings.shortcuts.unassigned").to_owned(), p.muted, UNASSIGNED)
    } else {
        (keys.clone(), p.text, KEYS_BUTTON)
    };
    let id = e.id.clone();
    let button = row().key(format!("shortcut-button-{}", e.id)).role("button").label(format!("{}: {words}", e.title)).h(24.0).px(9.0).center_y().radius(4.0)
        .fill(if recording { p.pressed } else { rune::Color::TRANSPARENT })
        .hover(if recording { p.pressed } else { p.hover })
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let st = &mut s.settings_pages.shortcuts;
            st.capture = if st.capture.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
            st.filter.focused = false;
            s.settings_pages.find.query.focused = false;
        })
        .child(text(words).font(font).color(ink));
    let id = e.id.clone();
    let unbind = (recording && !keys.is_empty()).then(|| {
        row().key("shortcut-unbind").role("button").label_static(t!("settings.shortcuts.unbind")).h(24.0).px(9.0).center_y().radius(4.0).hover(p.danger_bg).press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                bind(s, &id, "none");
                s.settings_pages.shortcuts.capture = None;
            })
            .child(text(t!("settings.shortcuts.unbind")).font(KEYS).color(p.danger))
    });
    row().gap(4.0).center_y().children(unbind).child(button)
}

/// Saves `spec` as command `id`'s keys and puts it to work.
fn bind(s: &mut App, id: &str, spec: &str) {
    let (id, spec) = (id.to_owned(), spec.to_owned());
    s.change_settings(|x| {
        x.keybinds.insert(id, spec);
    });
    overrides::apply(&s.settings.keybinds);
}

fn reset(s: &mut App) {
    s.settings_pages.shortcuts.capture = None;
    s.change_settings(|x| x.keybinds.clear());
    overrides::apply(&s.settings.keybinds);
    crate::status_toast::say(s, t!("settings.shortcuts.reset.toast.title"), t!("settings.shortcuts.reset.toast.description"));
}

/// The chord `ev` is, as a keybind names it: `mod` for ⌘ on macOS and Ctrl elsewhere; the key
/// a shifted character's key types without Shift. None for a key alone a chord can't end in.
fn chord_of(ev: &KeyEvent, tx: &Tx) -> Option<Chord> {
    let mac = KeyPlatform::current() == KeyPlatform::Mac;
    let key = match &ev.key {
        Key::Command(c) => ChordKey::Char(*c),
        Key::Text(t) => {
            let mut chars = t.chars();
            let c = chars.next().filter(|_| chars.next().is_none())?;
            let c = if c == ' ' { ' ' } else { tx.key_base().filter(|b| b.is_ascii() && !b.is_whitespace()).unwrap_or(c) };
            ChordKey::Char(c.to_ascii_lowercase())
        }
        Key::Enter => ChordKey::Enter,
        Key::Tab => ChordKey::Tab,
        Key::Backspace => ChordKey::Backspace,
        Key::Delete => ChordKey::Delete,
        Key::Insert => ChordKey::Insert,
        Key::Home => ChordKey::Home,
        Key::End => ChordKey::End,
        Key::PageUp => ChordKey::PageUp,
        Key::PageDown => ChordKey::PageDown,
        Key::Left => ChordKey::Left,
        Key::Right => ChordKey::Right,
        Key::Up => ChordKey::Up,
        Key::Down => ChordKey::Down,
        Key::Function(n) => ChordKey::Function(*n),
        _ => return None,
    };
    let m = ev.mods;
    let mut chord = Chord::new(key);
    let primary = if mac { m.cmd || m.logo } else { m.cmd || m.ctrl };
    if primary {
        chord = chord.with(ChordMod::Primary);
    }
    if mac && m.ctrl {
        chord = chord.with(ChordMod::Ctrl);
    }
    if !mac && m.logo {
        chord = chord.with(ChordMod::Super);
    }
    if m.alt {
        chord = chord.with(ChordMod::Alt);
    }
    if m.shift {
        chord = chord.with(ChordMod::Shift);
    }
    Some(chord)
}

/// The recording's keys (`transitionKeybindCapture`), and typing on the page into its search.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.settings_pages.page != super::Page::Shortcuts {
        return false;
    }
    let Some(id) = s.settings_pages.shortcuts.capture.clone() else { return type_to_search(s, ev, tx) };
    tx.instant();
    let bare = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.shift || ev.mods.logo);
    match &ev.key {
        Key::Escape => {
            s.settings_pages.shortcuts.capture = None;
            return true;
        }
        Key::Backspace | Key::Delete if bare => {
            bind(s, &id, "none");
            s.settings_pages.shortcuts.capture = None;
            return true;
        }
        Key::Modifier { .. } => return true,
        _ => {}
    }
    let Some(chord) = chord_of(ev, tx) else { return true };
    let platform = KeyPlatform::current();
    let taken: Vec<String> = entries(s).into_iter().filter(|e| e.id != id && e.keybind.chords().iter().any(|c| c.same_keys(&chord, platform))).map(|e| e.title).collect();
    if !taken.is_empty() {
        crate::status_toast::say(s, t!("settings.shortcuts.conflict.title"), t!("settings.shortcuts.conflict.description", keybind = crate::keymap::label(chord), titles = taken.join(", ")));
        return true;
    }
    bind(s, &id, &overrides::spec(chord));
    s.settings_pages.shortcuts.capture = None;
    true
}

/// A character typed with no field focused goes to the page's search (`SettingsSearchField`).
fn type_to_search(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Key::Text(t) = &ev.key else { return false };
    if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo || t.trim().is_empty() || tx.focused().is_some() {
        return false;
    }
    tx.instant();
    let f = &mut s.settings_pages.shortcuts.filter;
    let text = format!("{}{t}", f.text);
    *f = TextEdit::new(&text);
    f.focused = true;
    true
}

/// Runs a palette command the user bound in Settings (one with no row in the keymap table).
pub fn run_bound(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some((id, _)) = overrides::commands().into_iter().find(|(_, k)| k.matches_tx(ev, tx)) else { return false };
    if !s.commands().iter().any(|c| c.id == id) {
        return false;
    }
    tx.instant();
    s.run_command(&id, tx);
    true
}
