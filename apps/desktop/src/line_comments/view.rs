//! A line's comment boxes, as Desktop draws them.
//!
//! Files and the wide Changes preview use `LineCommentEditor`'s default layout and the
//! `LineComment` display card (`ui/src/data-display/line-comment/line-comment.tsx`,
//! `line-comment.css`), one under another up to 400 wide: `layer-01` on `--v2-elevation-raised`,
//! 6 round, 12 in. The editor is a four-line field (13 at 440 on 16) over Cancel (ghost) and
//! Comment, or Save for an edit (contrast); the card is the comment with a `...` menu (Edit,
//! Delete), and a click on it opens its edit.
//!
//! The narrow Changes list uses the older inline ones (`session-ui/src/components/line-comment.tsx`,
//! `line-comment-styles.ts`): the card says which lines it's on, and the editor is a bordered
//! three-line field over "Commenting on line N" and its two actions.
//!
//! The field is the composer quote editor's (`quote::comment`'s `Field` machine, its keys and
//! `@` list): Return comments (nothing when empty), Shift+Return breaks the line, Escape cancels.

use std::rc::Rc;

use rune::{Color, El, Icon, TextStyle, Tx, col, el, icon, row, text, text_input};

use super::machine::{Draft, Event};
use super::{LineComment, Spot, Surface};
use crate::App;
use crate::palette::{Elevate as _, Pal};
use crate::quote::comment::{self as field, FieldEvent, FieldKey};
use session_view::dock::{Variant, button};

/// `line-comment-v2-textarea` and `-text`: 13 at 440 on `--line-height-compact`.
const V2_TEXT: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// `line-comment-text` (v1): `--font-size-base` on `--line-height-x-large`.
const V1_TEXT: TextStyle = TextStyle::new(14.0, 440.0).lh(25.2);
/// `line-comment-label`, `-editor-label`, `-action` and `-textarea` (v1): 13 on 19.5.
const V1_LABEL: TextStyle = TextStyle::new(13.0, 530.0).lh(19.5);
const V1_FIELD: TextStyle = TextStyle::new(13.0, 440.0).lh(19.5);
const V1_ACTION: TextStyle = TextStyle::new(13.0, 530.0).lh(24.0);

/// `LineCommentOverflowIcon`.
static OVERFLOW: Icon = Icon {
    name: "line-comment-overflow",
    view: 16.0,
    body: r#"<path d="M2.5 7.5H3.5V8.5H2.5V7.5Z" stroke="currentColor"/><path d="M7.5 7.5H8.5V8.5H7.5V7.5Z" stroke="currentColor"/><path d="M12.5 7.5H13.5V8.5H12.5V7.5Z" stroke="currentColor"/>"#,
};

/// `dot-grid` (`additional-icons.ts`).
static DOT_GRID: Icon = Icon {
    name: "dot-grid",
    view: 20.0,
    body: r#"<path d="M2.08398 9.16602H3.75065V10.8327H2.08398V9.16602Z" fill="currentColor"/><path d="M10.834 9.16602H9.16732V10.8327H10.834V9.16602Z" fill="currentColor"/><path d="M16.2507 9.16602H17.9173V10.8327H16.2507V9.16602Z" fill="currentColor"/><path d="M2.08398 9.16602H3.75065V10.8327H2.08398V9.16602Z" stroke="currentColor"/><path d="M10.834 9.16602H9.16732V10.8327H10.834V9.16602Z" stroke="currentColor"/><path d="M16.2507 9.16602H17.9173V10.8327H16.2507V9.16602Z" stroke="currentColor"/>"#,
};

/// The boxes under line `line` on `side` of `spot`'s file: its saved comments in the order they
/// were written (one being edited as its editor), then the new comment's editor.
pub fn annotations(s: &App, p: &Pal, spot: &Spot, side: rune_kit::diff::Side, line: u32) -> Vec<El> {
    let Some(session) = s.line_comments.session(&spot.session) else { return draft_box(s, p, spot, side, line).into_iter().collect() };
    let mode = session.mode(spot.surface, &spot.path);
    let narrow = spot.surface == Surface::Narrow;
    let mut out: Vec<El> = session
        .on_file(&spot.path)
        .filter(|c| c.selection.anchor_side() == side && c.selection.anchor_line() == line)
        .map(|c| match mode.editing(&c.id) {
            Some(draft) if narrow => editor_v1(s, p, spot, draft, c.selection.label(), opencode_i18n::t!("common.save"), Some(&c.id)),
            Some(draft) => editor_v2(s, p, spot, draft, opencode_i18n::t!("common.save"), Some(&c.id)),
            None if narrow => card_v1(p, spot, c),
            None => card_v2(p, spot, c),
        })
        .collect();
    out.extend(draft_box(s, p, spot, side, line));
    out
}

fn draft_box(s: &App, p: &Pal, spot: &Spot, side: rune_kit::diff::Side, line: u32) -> Option<El> {
    let mode = s.line_comments.mode(spot);
    let range = mode.draft_range().filter(|r| r.anchor_side() == side && r.anchor_line() == line)?;
    let draft = mode.draft()?;
    Some(if spot.surface == Surface::Narrow {
        editor_v1(s, p, spot, draft, range.label(), opencode_i18n::t!("ui.lineComment.submit"), None)
    } else {
        editor_v2(s, p, spot, draft, opencode_i18n::t!("ui.lineComment.submit"), None)
    })
}

/// The field both layouts share: the draft bound to the mode at `spot`, its keys stepping the
/// field's machine, and `@` searching files where the view offers it (Files and the narrow list;
/// the wide preview doesn't).
fn field_input(s: &App, p: &Pal, spot: &Spot, draft: &Draft, style: TextStyle, placeholder: &str) -> El {
    let mention = spot.surface != Surface::Review;
    let (edit, change, key) = (spot.clone(), spot.clone(), spot.clone());
    text_input(&draft.text, style)
        .key("line-comment-field")
        .label(opencode_i18n::t!("ui.lineComment.submit"))
        .color(p.text)
        .w_full()
        .placeholder(placeholder.to_owned(), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .spellcheck(s.settings.spellcheck)
        .dir(rune::Direction::Auto)
        .on_edit(move |s: &mut App| s.line_comments.draft_text_mut(&edit))
        .on_input_change(move |s: &mut App, tx: &mut Tx| {
            if !mention {
                return;
            }
            let token = s.line_comments.mode(&change).draft().and_then(|d| field::token(&d.text));
            let now = tx.clock();
            s.line_comment_field(&change, FieldEvent::Synced { token, now }, tx);
        })
        .on_input_key(move |s: &mut App, ev, tx| {
            let k = crate::quote::editor::key_of(ev, tx);
            if k == FieldKey::Other {
                return false;
            }
            let config = s.line_comment_config(&key);
            s.line_comment_field(&key, FieldEvent::Key { key: k, config }, tx)
        })
}

fn mention_list(p: &Pal, spot: &Spot, draft: &Draft) -> Option<El> {
    let spot = spot.clone();
    crate::quote::editor::mention_rows(p, &draft.field.mention, Rc::new(move |s: &mut App, e: FieldEvent, tx: &mut Tx| {
        s.line_comment_field(&spot, e, tx);
    }))
}

/// `button-v2` small: 24 tall, 9 in, 4 round.
fn small_button(p: &Pal, key: &str, label: &str, variant: Variant, live: bool, press: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    let b = button(p, key, label, variant, None, live).h(24.0).px(9.0).radius(4.0);
    if live { b.on_press_tx(press) } else { b }
}

/// `LineCommentEditor`, default layout.
fn editor_v2(s: &App, p: &Pal, spot: &Spot, draft: &Draft, submit: &str, editing: Option<&str>) -> El {
    let input = field_input(s, p, spot, draft, V2_TEXT, opencode_i18n::t!("ui.lineComment.contextPlaceholder")).h(64.0);
    let can = !draft.text.text.trim().is_empty();
    let (cancel, send) = (spot.clone(), spot.clone());
    let actions = row()
        .key("actions")
        .gap(8.0)
        .center_y()
        .child(small_button(p, "line-comment-cancel", opencode_i18n::t!("ui.lineComment.cancel"), Variant::Ghost, true, move |s: &mut App, tx: &mut Tx| {
            s.line_comment_field(&cancel, FieldEvent::CancelRequested, tx);
        }))
        .child(small_button(p, "line-comment-submit", submit, Variant::Contrast, can, move |s: &mut App, tx: &mut Tx| {
            let config = s.line_comment_config(&send);
            s.line_comment_field(&send, FieldEvent::SubmitRequested { config }, tx);
        }));
    col()
        .key(editing.map_or_else(|| "line-comment-draft".to_owned(), |id| format!("line-comment-edit-{id}")))
        .w_full()
        .max_w(400.0)
        .padding(12.0)
        .gap(12.0)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.raised)
        .preserve_focus_on_press(true)
        .on_press(|_: &mut App| {})
        .child(col().key("line-comment-field-col").w_full().gap(8.0).child(input).children(mention_list(p, spot, draft)))
        .child(row().key("footer").w_full().gap(4.0).center_y().child(el().grow(1.0)).child(actions))
}

/// `LineComment`, display: the comment, and its `...` menu; a click anywhere on it opens its
/// edit, and the pointer over it lights its lines (`comment.hovered`).
fn card_v2(p: &Pal, spot: &Spot, c: &LineComment) -> El {
    let (open, hover) = (spot.clone(), spot.clone());
    let (id, range, saved) = (c.id.clone(), c.selection, c.comment.clone());
    let edit_click = Event::EditOpened { id, range, text: saved };
    let tools = col().key("tools").h(25.33).child(menu_button(p, spot, c, false));
    row()
        .key(format!("line-comment-{}", c.id))
        .role("button")
        .label(c.comment.clone())
        .w_full()
        .max_w(400.0)
        .padding(12.0)
        .gap(8.0)
        .items_start()
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.raised)
        .hover(p.hover)
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        .cursor(rune::Cursor::Pointer)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                s.line_comment(&hover, Event::CommentHovered(range), tx);
            } else {
                tx.live_only();
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.line_comment(&open, edit_click.clone(), tx))
        .child(col().key("column").grow(1.0).shrink().min_w(0.0).gap(8.0).child(text(c.comment.clone()).key("text").font(V2_TEXT).color(p.text).w_full().wrap_anywhere()))
        .child(tools)
}

/// The card's `...`: Edit and Delete (`FileCommentMenu`, `ReviewCommentMenu`).
fn menu_button(p: &Pal, spot: &Spot, c: &LineComment, v1: bool) -> El {
    let anchor = format!("line-comment-menu-{}", c.id);
    let (spot, id, range, saved) = (spot.clone(), c.id.clone(), c.selection, c.comment.clone());
    let label = opencode_i18n::t!("common.moreOptions");
    let b = if v1 {
        // A ghost small IconButton whose glyph `[data-slot=icon-svg] { color: var(--white) }`
        // whitens (`line-comment-styles.ts:39-41`).
        el().key(anchor.clone()).size(24.0, 24.0).radius(6.0).center().child(icon(&DOT_GRID, 16.0).color(Color::WHITE))
    } else {
        el().key(anchor.clone()).size(20.0, 20.0).padding(2.0).radius(4.0).center().child(icon(&OVERFLOW, 16.0).color(p.icon_muted))
    };
    b.role("button")
        .label(label)
        .cursor(rune::Cursor::Pointer)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            use crate::popup::{Item, Menu};
            // `menu-v2` at its 160 floor, its end under the trigger's.
            let menu = Menu::new(label).min_width(160.0).align(rune::Align::End).item(Item::new("edit", opencode_i18n::t!("common.edit"))).item(Item::new("delete", opencode_i18n::t!("common.delete")));
            let (spot, id, saved) = (spot.clone(), id.clone(), saved.clone());
            crate::popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, tx| match picked {
                Some("edit") => s.line_comment(&spot, Event::EditOpened { id, range, text: saved }, tx),
                Some("delete") => s.line_comment(&spot, Event::CommentDeleted { id }, tx),
                _ => {}
            });
        })
}

/// `--surface-raised-stronger-non-alpha` and `--border-weak-base`, the inline boxes' face and
/// their 0.5 ring (`--shadow-xxs-border`).
fn v1_face(p: &Pal) -> (Color, Color) {
    if p.dark { (hex("#1c1c1c"), hex("#2e2e2e")) } else { (Color::WHITE, hex("#dbdbdb")) }
}

/// The inline `LineComment`: the comment, its menu, and "Comment on line N"; a click opens it
/// (`comment.toggled`), the pointer over it lights its lines.
fn card_v1(p: &Pal, spot: &Spot, c: &LineComment) -> El {
    let (face, ring) = v1_face(p);
    let (toggle, hover) = (spot.clone(), spot.clone());
    let (id, range) = (c.id.clone(), c.selection);
    let label = format!("{}{}{}", opencode_i18n::t!("ui.lineComment.label.prefix"), range.label(), opencode_i18n::t!("ui.lineComment.label.suffix"));
    col()
        .key(format!("line-comment-{}", c.id))
        .w_full()
        .padding(12.0)
        .gap(6.0)
        .radius(8.0)
        .fill(face)
        .box_shadow(Pal::ring_out(ring))
        .cursor(rune::Cursor::Pointer)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                s.line_comment(&hover, Event::CommentHovered(range), tx);
            } else {
                tx.live_only();
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.line_comment(&toggle, Event::CommentToggled { id: id.clone(), range }, tx))
        .child(
            row()
                .key("head")
                .w_full()
                .gap(8.0)
                .items_start()
                .child(text(c.comment.clone()).key("text").font(V1_TEXT).color(p.text).grow(1.0).shrink().min_w(0.0).wrap_anywhere())
                .child(col().key("tools").h(27.33).child(menu_button(p, spot, c, true))),
        )
        .child(text(label).key("label").font(V1_LABEL).color(p.faint).w_full().wrap_anywhere())
}

/// The inline `LineCommentEditor`: a bordered field on `surface-base`, ringed in the focus blue
/// while it has the keyboard, over "Commenting on line N", Cancel and the submit.
fn editor_v1(s: &App, p: &Pal, spot: &Spot, draft: &Draft, lines: String, submit: &str, editing: Option<&str>) -> El {
    let (face, ring) = v1_face(p);
    let focused = draft.text.focused;
    let field_bg = if p.dark { Color::WHITE.alpha(0.031) } else { Color::BLACK.alpha(0.031) };
    let border = if p.dark { Color::WHITE.alpha(0.1) } else { Color::BLACK.alpha(0.1) };
    let mut input = col()
        .key("field-box")
        .w_full()
        .h(75.83)
        .padding(8.0)
        .radius(6.0)
        .fill(field_bg)
        .box_shadow(Pal::ring_in(border))
        .child(field_input(s, p, spot, draft, V1_FIELD, opencode_i18n::t!("ui.lineComment.placeholder")).h_full());
    if focused {
        // `--shadow-xs-border-select`: a 1 px focus ring in a 3 px wash.
        let ring = |spread: f32, color: Color| rune::Shadow { spread, color, ..rune::Shadow::default() };
        input = input.elevate([ring(1.0, hex("#054dfd").alpha(0.99)), ring(3.0, hex("#d2e1fd").alpha(0.5)), rune::Shadow::default(), rune::Shadow::default()]);
    }
    let can = !draft.text.text.trim().is_empty();
    let (cancel, send) = (spot.clone(), spot.clone());
    let action = |key: &str, label: &str, primary: bool, live: bool, press: Box<dyn Fn(&mut App, &mut Tx)>| {
        let (fill, fg, edge) = if primary { (p.contrast, p.q.contrast_text, p.contrast) } else { (Color::TRANSPARENT, p.text, border) };
        let b = row().key(key.to_owned()).role("button").label(label.to_owned()).h(28.0).px(10.0).center_y().radius(6.0).fill(fill).box_shadow(Pal::ring_in(edge)).child(text(label.to_owned()).font(V1_ACTION).color(fg));
        if live { b.cursor(rune::Cursor::Pointer).on_press_tx(move |s: &mut App, tx: &mut Tx| press(s, tx)) } else { b.opacity(0.5) }
    };
    let label = format!("{}{}{}", opencode_i18n::t!("ui.lineComment.editorLabel.prefix"), lines, opencode_i18n::t!("ui.lineComment.editorLabel.suffix"));
    col()
        .key(editing.map_or_else(|| "line-comment-draft".to_owned(), |id| format!("line-comment-edit-{id}")))
        .w_full()
        .padding(8.0)
        .radius(14.0)
        .fill(face)
        .box_shadow(Pal::ring_out(ring))
        .preserve_focus_on_press(true)
        .on_press(|_: &mut App| {})
        .child(
            col()
                .key("editor")
                .w_full()
                .gap(8.0)
                .child(input)
                .children(mention_list(p, spot, draft))
                .child(
                    row()
                        .key("actions")
                        .w_full()
                        .h(28.0)
                        .pl(8.0)
                        .gap(8.0)
                        .center_y()
                        .child(text(label).key("label").font(V1_LABEL).color(p.faint).grow(1.0).shrink().min_w(0.0).truncate())
                        .child(action("line-comment-cancel", opencode_i18n::t!("ui.common.cancel"), false, true, Box::new(move |s: &mut App, tx: &mut Tx| {
                            s.line_comment_field(&cancel, FieldEvent::CancelRequested, tx);
                        })))
                        .child(action("line-comment-submit", submit, true, can, Box::new(move |s: &mut App, tx: &mut Tx| {
                            let config = s.line_comment_config(&send);
                            s.line_comment_field(&send, FieldEvent::SubmitRequested { config }, tx);
                        }))),
                ),
        )
}

fn hex(s: &str) -> Color {
    rune::color::hex(s).unwrap_or(Color::BLACK)
}