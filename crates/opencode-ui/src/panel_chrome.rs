//! The file panel frame Review and Files share (`SessionFilePanelV2`, `SessionReviewV2Sidebar`,
//! `session-review-v2.css`, `file-tree.css`): the tree toggle pinned at the panel's top left, the
//! 48 px sidebar header and toolbar, the borderless filter field, and 28 px tree rows.

use rune::{Anim, Color, Cursor, El, Icon, KeyEvent, TextEdit, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::diff::FileStatus;
use rune_theme_opencode::icons as ic;
use rune_theme_opencode::tree_icons::TreeIcon;
use crate::palette::{Pal, kit};
use crate::style::UI;

const INSTANT: Anim = Anim::INSTANT;
/// The header and toolbar height, and where their content starts past the toggle
/// (`data-sidebar-toggle-gap="10"`).
pub const BAR_H: f32 = 48.0;
pub const TOGGLE_INSET: f32 = 54.0;
pub const ROW_H: f32 = 28.0;
/// Each tree level's indent in Desktop's Pierre tree.
pub const INDENT: f32 = 22.0;

pub static FILETREE: Icon = Icon {
    name: "panel-filetree",
    view: 16.0,
    body: r#"<path d="M2.5 1.5V12.2484H6.75M2.5 4.74838H6.75" stroke="currentColor" fill="none"/><rect x="8.5" y="3.2168" width="6" height="3" fill="none" stroke="currentColor"/><rect x="8.5" y="10.75" width="6" height="3" fill="none" stroke="currentColor"/>"#,
};
static SEARCH: Icon = Icon {
    name: "panel-filter-search",
    view: 14.0,
    body: r#"<path d="M12.25 12.25L10.0625 10.0625M11.0833 6.41667C11.0833 8.994 8.994 11.0833 6.41667 11.0833C3.83934 11.0833 1.75 8.994 1.75 6.41667C1.75 3.83934 3.83934 1.75 6.41667 1.75C8.994 1.75 11.0833 3.83934 11.0833 6.41667Z" stroke="currentColor" stroke-linecap="square" fill="none"/>"#,
};
static CHEVRON_DOWN: Icon = Icon {
    name: "panel-tree-chevron-down",
    view: 16.0,
    body: r#"<path d="M5 6.5L8 9.5L11 6.5" stroke="currentColor" fill="none"/>"#,
};
static CHEVRON_RIGHT: Icon = Icon {
    name: "panel-tree-chevron-right",
    view: 16.0,
    body: r#"<path d="M6.5 5L9.5 8L6.5 11" stroke="currentColor" fill="none"/>"#,
};

fn sidebar_toggle_shell(p: &Pal, key: &'static str, open: bool, disabled: bool) -> El {
    let b = el()
        .key(key)
        .absolute(8.0, 9.5)
        .size(36.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if open { p.layer2 } else { Color::TRANSPARENT })
        .role("button")
        .label_static(opencode_i18n::t!("ui.sessionReviewV2.toggleSidebar"))
        .help(opencode_i18n::t!("ui.sessionReviewV2.toggleSidebar"))
        .help_keys(crate::keymap::Bind::TreeToggle.keycaps())
        .child(icon(&FILETREE, 16.0).color(p.icon));
    // A disabled IconButton draws at half opacity, its expanded fill too.
    if disabled {
        return b.opacity(0.5).inert();
    }
    b.cursor(Cursor::Pointer)
        .hover(if open { p.layer2 } else { p.hover })
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
}

/// The tree toggle (`SessionReviewV2SidebarToggle`): 36 × 28 at 8, 9.5 from the panel's corner,
/// on `layer-02` while the tree shows.
pub fn sidebar_toggle<S: 'static>(
    p: &Pal,
    key: &'static str,
    open: bool,
    disabled: bool,
    press: impl Fn(&mut S, &mut Tx) + 'static,
) -> El {
    let b = sidebar_toggle_shell(p, key, open, disabled);
    if disabled {
        return b;
    }
    b.on_press_tx(press)
}

/// The sidebar's header: its title past the toggle, then whatever ends it (stats, actions).
pub fn sidebar_header(key: &'static str, title: El, end: Option<El>) -> El {
    row()
        .key(key)
        .w_full()
        .h(BAR_H)
        .pl(TOGGLE_INSET)
        .pr(16.0)
        .gap(8.0)
        .center_y()
        .child(title.shrink().min_w(0.0))
        .child(el().grow(1.0))
        .children(end)
}

fn filter_field_input(p: &Pal, key: &str, edit: &TextEdit, placeholder: &str, spellcheck: bool) -> El {
    crate::style::field(edit, UI)
        .key(key.to_owned())
        // Named by its placeholder, which goes once a letter is typed.
        .label(placeholder.to_owned())
        .spellcheck(spellcheck)
        .placeholder(placeholder.to_owned(), p.faint)
        .selection_style(p.field_selection)
        .color(p.text)
        .grow(1.0)
        .shrink()
}

fn filter_clear_shell(p: &Pal) -> El {
    el().key("filter-clear")
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .role("button")
        .label_static(opencode_i18n::t!("ui.list.clearFilter"))
        .child(icon(&ic::CLOSE, 16.0).color(p.icon_muted))
}

fn filter_field_frame(p: &Pal, hovered_key: &str, focused: bool, input: El, clear: Option<El>) -> El {
    let field = row()
        .key(hovered_key.to_owned())
        .w_full()
        .h(28.0)
        .mt(3.0)
        .pl(8.0)
        .pr(4.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(if focused { p.hover } else { Color::TRANSPARENT })
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .border(1.0, if focused { kit(p).c.border_focus } else { Color::TRANSPARENT })
        .motion(rune::Spring::NONE)
        .child(icon(&SEARCH, 14.0).color(p.icon_muted))
        .child(input)
        .children(clear);
    col().w_full().px(8.0).pb(4.0).child(field)
}

/// The sidebar's filter (`TextInput` on the invisible surface): 28 px, no fill until hovered or
/// focused, then the hover overlay and, focused, a 1 px focus ring. It marks misspelled words
/// as Desktop's `TextInput` does, when `spellcheck` (Settings › General › Spellcheck).
#[allow(clippy::too_many_arguments)]
pub fn filter_field<S: 'static>(
    p: &Pal,
    key: &str,
    edit: &TextEdit,
    hovered_key: &str,
    placeholder: &str,
    on_edit: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static,
    on_submit: impl Fn(&mut S, &mut Tx) + 'static,
    clear: impl Fn(&mut S, &mut Tx) + 'static,
    spellcheck: bool,
    on_key: impl Fn(&mut S, &KeyEvent, &mut Tx) -> bool + 'static,
) -> El {
    let input = filter_field_input(p, key, edit, placeholder, spellcheck)
        .on_edit(on_edit)
        .on_submit(on_submit)
        .on_input_key(on_key);
    let clear_btn = (!edit.text.is_empty()).then(|| filter_clear_shell(p).on_press_tx(clear));
    filter_field_frame(p, hovered_key, edit.focused, input, clear_btn)
}

/// A tree row (`file-tree-v2-row`): a folder shows only its chevron, 1 px low, a file its
/// `@pierre/trees` icon, in its type's colour on the selected row; the selected row takes the
/// pressed overlay and the base text colour.
pub fn tree_row(p: &Pal, name: &str, depth: u32, expanded: Option<bool>, file_glyph: Option<TreeIcon>, selected: bool) -> El {
    tree_row_in(p, name, depth, expanded, file_glyph, selected, UI)
}

/// Files' workspace tree (`@pierre/trees` as Desktop styles it): 12 at 500.
pub const PIERRE: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);

/// [`tree_row`] with its name in `font`.
pub fn tree_row_in(p: &Pal, name: &str, depth: u32, expanded: Option<bool>, file_glyph: Option<TreeIcon>, selected: bool, font: TextStyle) -> El {
    let lead = match (expanded, file_glyph) {
        (Some(open), _) => icon(if open { &CHEVRON_DOWN } else { &CHEVRON_RIGHT }, 16.0)
            .color(if selected { p.text } else { p.icon_muted })
            .offset(0.0, 1.0),
        (None, Some(t)) => icon(t.icon(), 16.0).color(if selected { t.color(p.dark) } else { p.icon_muted }),
        (None, None) => el().size(16.0, 16.0),
    };
    let base = match expanded {
        Some(open) => row().expanded(open),
        None => row(),
    };
    base.w_full()
        .h(ROW_H)
        .pl(8.0 + depth as f32 * INDENT)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(if selected { p.pressed } else { Color::TRANSPARENT })
        .hover(if selected { p.pressed } else { p.hover })
        .hover_motion(INSTANT, INSTANT)
        .cursor(Cursor::Pointer)
        .role("treeitem")
        .label(name.to_owned())
        .level(depth + 1)
        .focus_selected(selected)
        .child(lead)
        .child(
            text(name.to_owned())
                .font(font)
                .color(if selected { p.text } else { p.muted })
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate_fade(),
        )
}

/// A file's change letter at a row's end (`file-tree-v2-change`).
pub fn change_letter(p: &Pal, status: FileStatus) -> El {
    let (letter, color) = match status {
        FileStatus::Added => ("A", p.success),
        FileStatus::Deleted => ("D", p.danger),
        _ => ("M", p.info),
    };
    el().w(16.0).center().child(text(letter).font(CHANGE).color(color))
}

const CHANGE: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).features("'tnum' 1, 'lnum' 1");
