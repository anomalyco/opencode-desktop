//! Appearance's font field: the row's `TextInput` in its own face, the bundled and installed
//! families dropped under it when the field is clicked or its chevron, `↓` or typing opens the
//! list, filtered by what the field holds. Official OpenCode has plain font inputs here
//! (`packages/app/src/settings/appearance/appearance.tsx`); the list is this app's own addition.

use opencode_i18n::t;
use opencode_ui::palette::{Pal, kit};
use opencode_ui::popup::{self, Menu, MenuButton as _, PopupHost};
use rune::{El, Key, KeyEvent, TextEdit, TextStyle, Tx, icon, row};
use rune_kit::TextInput;
use rune_theme_opencode::icons as ic;

use super::State;
use crate::host::SettingsHost;

/// `use-typed`: the list's one row when nothing matched, which changes nothing (the typed name
/// is already in effect).
pub(super) const USE_TYPED: &str = "use-typed";

/// Appearance's font row: `edit` drawn as a 220 pt field in `style`'s face, the list `menu`
/// builds dropping under it (`popup::open_attached`) when the field is clicked, a chevron or `↓`
/// is pressed, or it is typed in. Typing applies through `change` as it always has and filters
/// the list; `b:`/`s:` row ids write their name into the field and apply it; while the list is
/// open Up/Down/PageUp/PageDown/Enter/Escape move and pick in it without focus leaving the field.
#[allow(clippy::too_many_arguments)]
pub fn font_field<H: SettingsHost + PopupHost + 'static>(
    s: &H,
    p: &Pal,
    key: &'static str,
    label: &str,
    placeholder: &str,
    edit: &TextEdit,
    style: TextStyle,
    field: fn(&mut State) -> &mut TextEdit,
    change: fn(&mut dyn SettingsHost, &mut Tx),
    menu: fn(&H) -> Menu,
) -> El {
    let open = s.popup().open_at(key);
    // Its list open, the chevron turns up and keeps the wash, as the theme select's trigger does
    // (`Select`'s trigger: `chevron_up` while expanded, the hover fill).
    let chevron = row()
        .key(format!("{key}-chevron"))
        .role("button")
        .label_static(t!("rune.settings.fonts.show"))
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .tab_index(-1)
        .preserve_focus_on_press(true)
        .fill(if open { p.hover } else { rune::Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .child(
            icon(&ic::CHEVRON_DOWN, 16.0)
                .rotate(if open { std::f32::consts::PI } else { 0.0 })
                .color(if open { p.icon } else { p.icon_muted }),
        )
        .menu_button(key.to_owned(), move |s: &mut H, tx: &mut Tx| show(s, tx, key, field, change, menu));
    TextInput::new(edit)
        .placeholder(placeholder.to_owned())
        .label(label.to_owned())
        .width(if crate::rows_wrap() { None } else { Some(220.0) })
        .text_style(style)
        .height(28.0)
        .trailing(chevron)
        .on_edit(move |s: &mut H| field(s.settings_pages_mut()))
        .on_change(move |s: &mut H, tx: &mut Tx| {
            change(s, tx);
            if s.popup().open_at(key) {
                popup::edited(s, tx);
            } else {
                show(s, tx, key, field, change, menu);
            }
        })
        .on_key(move |s: &mut H, ev: &KeyEvent, tx: &mut Tx| font_key(s, ev, tx, key, field, change, menu))
        .on_focus_change(move |s: &mut H, focused: bool, tx: &mut Tx| {
            if !focused {
                if s.popup().open_at(key) {
                    popup::dismiss(s, tx);
                }
            } else if !tx.focus_visible() && !s.popup().open_at(key) {
                // A click anywhere in the field opens its list, a target as wide as the field
                // itself; the keyboard (Tab) only focuses it.
                show_next(s, tx, key, field, change, menu);
            }
        })
        .build(kit(p))
        .key(key.to_owned())
}

/// Opens the list under the field's `key`, the field focused so typing filters it at once.
fn show<H: SettingsHost + PopupHost + 'static>(
    s: &mut H,
    tx: &mut Tx,
    key: &'static str,
    field: fn(&mut State) -> &mut TextEdit,
    change: fn(&mut dyn SettingsHost, &mut Tx),
    menu: fn(&H) -> Menu,
) {
    field(s.settings_pages_mut()).focused = true;
    popup::open_attached(s, menu, key.to_owned(), tx, picker(field, change));
}

/// [`show`] on the next frame: the click that asked for it lands at once (its caret must), and
/// a list mounted in that frame would skip its entrance.
fn show_next<H: SettingsHost + PopupHost + 'static>(
    s: &mut H,
    tx: &mut Tx,
    key: &'static str,
    field: fn(&mut State) -> &mut TextEdit,
    change: fn(&mut dyn SettingsHost, &mut Tx),
    menu: fn(&H) -> Menu,
) {
    field(s.settings_pages_mut()).focused = true;
    popup::open_attached_next(s, menu, key.to_owned(), tx, picker(field, change));
}

/// What a picked row does: a `b:`/`s:` id writes its family into the field and applies it
/// (`use-typed` names nothing: the typed text is already in effect).
fn picker<H: SettingsHost + 'static>(
    field: fn(&mut State) -> &mut TextEdit,
    change: fn(&mut dyn SettingsHost, &mut Tx),
) -> impl FnOnce(&mut H, Option<&str>, &mut Tx) + 'static {
    move |s: &mut H, picked: Option<&str>, tx: &mut Tx| {
        let Some(name) = picked.and_then(|id| id.strip_prefix("b:").or_else(|| id.strip_prefix("s:"))) else { return };
        let edit = field(s.settings_pages_mut());
        *edit = TextEdit::new(name);
        edit.focused = true;
        change(s, tx);
    }
}

/// The field's keys: `↓` opens the list while it is closed; while it is open the arrows, Page
/// Up/Down, Return and Escape move, pick and close in it (an input method's composition keeps
/// its own keys).
fn font_key<H: SettingsHost + PopupHost + 'static>(
    s: &mut H,
    ev: &KeyEvent,
    tx: &mut Tx,
    key: &'static str,
    field: fn(&mut State) -> &mut TextEdit,
    change: fn(&mut dyn SettingsHost, &mut Tx),
    menu: fn(&H) -> Menu,
) -> bool {
    if field(s.settings_pages_mut()).is_composing() {
        return false;
    }
    let open = s.popup().open_at(key);
    match &ev.key {
        Key::Down if !open => {
            show(s, tx, key, field, change, menu);
            true
        }
        Key::Down | Key::Up | Key::PageUp | Key::PageDown | Key::Enter | Key::Escape if open => popup::key(s, ev, tx),
        _ => false,
    }
}
