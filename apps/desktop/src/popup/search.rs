//! A searchable drawn menu's field (`recently-closed-search`).

use rune::{El, Key, TextEdit, TextStyle, Tx, el, row};

use super::{choose, key, query_changed, refresh};
use crate::App;
use crate::palette::Pal;

/// A searchable menu's field and the 4pt around it (`recently-closed-search m-1`): Desktop's
/// 28pt `TextInput` with the magnifying glass, clear until hovered, focused or typed in, where it
/// takes the hover wash, and its clear button once there's a query.
pub(super) const SEARCH_H: f32 = 28.0 + 8.0;

pub(super) fn search_field(s: &App, p: &Pal, placeholder: &str) -> El {
    use rune_theme_opencode::icons as ic;
    let k = crate::palette::kit(p);
    let edit = &s.popup.query;
    let typed = !edit.text.is_empty();
    let input = rune::text_input(edit, INPUT)
        .key("popup-search-input")
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder(placeholder.to_owned(), p.faint)
        .caret(p.text, p.selection).selection_style(p.field_selection)
        .on_edit(|s: &mut App| &mut s.popup.query)
        .on_input_change(|s: &mut App, _: &mut Tx| query_changed(s))
        .on_input_key(|s: &mut App, ev, tx: &mut Tx| matches!(ev.key, Key::Up | Key::Down | Key::Escape | Key::Tab | Key::PageUp | Key::PageDown) && key(s, ev, tx))
        .on_submit(|s: &mut App, tx: &mut Tx| {
            refresh(s);
            if let Some(n) = s.popup.shown.as_ref().and_then(|m| m.active) {
                choose(s, n, tx);
            }
        });
    let mut field = row()
        .key("popup-search")
        .role("search")
        .w_full()
        .h(28.0)
        .pl(8.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .child(rune::icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(input);
    if typed || edit.focused {
        field = field.fill(p.hover);
    }
    if edit.focused {
        field = field.border(1.0, k.c.border_focus);
    }
    if typed {
        field = field.child(
            el().key("popup-search-clear")
                .role("button")
                .label_static(opencode_i18n::t!("common.clear"))
                .size(16.0, 16.0)
                .center()
                .on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.popup.query = TextEdit::new("");
                    s.popup.query.focused = true;
                    query_changed(s);
                })
                .child(rune::icon(&ic::CIRCLE_XMARK, 16.0).color(p.icon_faint).group_color(p.icon)),
        );
    }
    row().key("popup-search-wrap").w_full().padding(4.0).child(field)
}

/// The search field's text: 13/440, the compact line height.
const INPUT: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
