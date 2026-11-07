//! The file panel frame Review and Files share (`SessionFilePanelV2`, `SessionReviewV2Sidebar`,
//! `session-review-v2.css`, `file-tree.css`): the tree toggle pinned at the panel's top left, the
//! 48 px sidebar header and toolbar, the borderless filter field, and 28 px tree rows.

use rune::{Anim, Color, Cursor, El, Icon, TextEdit, Tx, col, el, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;
use rune_theme_opencode::tree_icons::TreeIcon;

use crate::App;
use crate::palette::{Pal, kit};
use crate::view::UI;

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

/// The tree toggle (`SessionReviewV2SidebarToggle`): 36 × 28 at 8, 9.5 from the panel's corner,
/// on `layer-02` while the tree shows.
pub fn sidebar_toggle(p: &Pal, key: &'static str, open: bool, disabled: bool, press: fn(&mut App, &mut Tx)) -> El {
    let b = el()
        .key(key)
        .absolute(8.0, 9.5)
        .size(36.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if open { p.layer2 } else { Color::TRANSPARENT })
        .role("button")
        .label("Toggle file tree")
        .help("Toggle file tree").help_keys(crate::keymap::Bind::TreeToggle.keycaps())
        .child(icon(&FILETREE, 16.0).color(p.icon));
    if disabled {
        return b.inert();
    }
    b.cursor(Cursor::Pointer)
        .hover(if open { p.layer2 } else { p.hover })
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(press)
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

/// The sidebar's filter (`TextInput` on the invisible surface): 28 px, no fill until hovered or
/// focused, then the hover overlay and, focused, a 1 px focus ring.
pub fn filter_field(
    p: &Pal,
    key: &'static str,
    edit: &TextEdit,
    hovered_key: &'static str,
    on_edit: fn(&mut App) -> &mut TextEdit,
    on_submit: fn(&mut App, &mut Tx),
    clear: fn(&mut App, &mut Tx),
) -> El {
    let focused = edit.focused;
    let field = row()
        .key(hovered_key)
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
        .child(
            text_input(edit, UI)
                .key(key)
                .placeholder("Filter files", p.faint)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .on_edit(on_edit)
                .on_submit(on_submit),
        )
        .children((!edit.text.is_empty()).then(|| {
            el().key("filter-clear")
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .cursor(Cursor::Pointer)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .role("button")
                .label("Clear filter")
                .on_press_tx(clear)
                .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted))
        }));
    col().w_full().px(8.0).pb(4.0).child(field)
}

/// A tree row (`file-tree-v2-row`): a folder shows only its chevron, 1 px low, a file its
/// `@pierre/trees` icon, in its type's colour on the selected row; the selected row takes the
/// pressed overlay and the base text colour.
pub fn tree_row(p: &Pal, name: &str, depth: u32, expanded: Option<bool>, file_glyph: Option<TreeIcon>, selected: bool) -> El {
    let lead = match (expanded, file_glyph) {
        (Some(open), _) => icon(if open { &CHEVRON_DOWN } else { &CHEVRON_RIGHT }, 16.0)
            .color(if selected { p.text } else { p.icon_muted })
            .offset(0.0, 1.0),
        (None, Some(t)) => icon(t.icon(), 16.0).color(if selected { t.color(p.dark) } else { p.icon_muted }),
        (None, None) => el().size(16.0, 16.0),
    };
    row()
        .w_full()
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
        .child(lead)
        .child(
            text(name.to_owned())
                .font(UI)
                .color(if selected { p.text } else { p.muted })
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .truncate_fade(),
        )
}

/// A file's change letter at a row's end (`file-tree-v2-change`).
pub fn change_letter(p: &Pal, status: rune_kit::diff::FileStatus) -> El {
    use rune_kit::diff::FileStatus;
    let (letter, color) = match status {
        FileStatus::Added => ("A", p.success),
        FileStatus::Deleted => ("D", p.danger),
        _ => ("M", p.info),
    };
    el().w(16.0).center().child(text(letter).font(CHANGE).color(color))
}

const CHANGE: rune::TextStyle = rune::TextStyle::new(11.0, 530.0).lh(11.0).features("'tnum' 1, 'lnum' 1");

/// A diff's file header (`session-review-v2-file-header`): its status letter, the file's
/// coloured icon, its name and its folder, 6 apart, then its `+N -M` 8 past them, 8 by 16 in;
/// a 1 px `border-base` rule under it.
pub fn file_header(p: &Pal, path: &str, status: rune_kit::diff::FileStatus, adds: usize, dels: usize) -> El {
    use rune_kit::diff::FileStatus;
    let (letter, color) = match status {
        FileStatus::Added => ("A", p.diff.add_text),
        FileStatus::Deleted => ("D", p.diff.del_text),
        _ => ("M", p.info),
    };
    let (dir, name) = opencode_kit::path::split(path);
    let title = row()
        .key("title")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(6.0)
        .center_y()
        .child(el().key("status").size(16.0, 16.0).center().child(text(letter).font(STATUS).color(color)))
        .child(icon(rune_theme_opencode::file_icons::file(path), 16.0).key("file-icon"))
        .child(text(name.to_owned()).key("name").font(FILE_NAME).color(p.text))
        .children((!dir.is_empty()).then(|| text(dir.to_owned()).key("dir").font(FILE_PATH).color(p.muted).shrink().min_w(0.0).truncate_fade()));
    // `DiffChanges` compact: both counts, 8 apart, in 11 px.
    let counts = row()
        .key("counts")
        .gap(8.0)
        .center_y()
        .children((adds + dels > 0).then(|| text(format!("+{adds}")).key("adds").font(COUNT).color(p.success)))
        .children((adds + dels > 0).then(|| text(format!("-{dels}")).key("dels").font(COUNT).color(p.danger)));
    row()
        .key("file-header")
        .w_full()
        .px(16.0)
        .py(8.0)
        .gap(8.0)
        .center_y()
        .fill(p.bg_base)
        .border_sides([0.0, 0.0, 1.0, 0.0], p.border)
        .child(title)
        .child(counts)
}

const STATUS: rune::TextStyle = rune::TextStyle::new(11.0, 530.0).lh(11.0).tracking(-0.04).features("'tnum' 1, 'lnum' 1");
const FILE_NAME: rune::TextStyle = rune::TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04).features("'tnum' 1, 'lnum' 1");
const FILE_PATH: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04).features("'tnum' 1, 'lnum' 1");
const COUNT: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(11.0).tracking(0.05);
