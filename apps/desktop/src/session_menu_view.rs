//! What the session menus and rename draw: the title field, Delete's confirmation, and a drawn
//! copy of a menu for the storybook and stills (native menus never draw headless).

use rune::{Color, El, Tx, col, el, row, spacer, text, text_input};

use crate::palette::Pal;
use crate::session_menu::{Line, Spot, Target};
use crate::view::{UI, UI_MED};
use crate::App;

/// `s`'s title field, in `style`, when `target` is the one being renamed. Outside a tab it hugs
/// its text, so what follows stays put and moves only as the title gets longer; one too long for
/// the room scrolls inside it and fades at the edge.
pub fn field(s: &App, p: &Pal, target: &Target, style: rune::TextStyle) -> Option<El> {
    if s.menus.rename.as_ref() != Some(target) {
        return None;
    }
    let input = text_input(&s.menus.edit, style).key("rename").single_line().truncate_fade().h(16.0);
    let input = if target.spot == Spot::Tab { input.grow(1.0) } else { input };
    Some(
        input
            .color(p.text)
            .caret(p.text, p.selection)
            .on_edit(|s: &mut App| &mut s.menus.edit)
            .on_input_key(|s: &mut App, ev, tx: &mut Tx| crate::session_menu::key(s, ev, tx))
            .on_submit(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.finish_rename(true);
            }),
    )
}

/// Delete's confirmation (`session.delete.*`): a scrim and a card, Cancel and the danger button.
pub fn confirm(s: &App, p: &Pal) -> Vec<El> {
    let Some(id) = &s.menus.confirm else { return vec![] };
    let name = s.session(id).map_or_else(|| "Session".to_owned(), crate::view::title_of);
    let button = |key: &str, label: &str, ink: Color, fill: Option<Color>| {
        let mut b = row().key(key.to_owned()).role("button").label(label.to_owned()).h(28.0).px(12.0).radius(6.0).center().hover(p.hover);
        if let Some(f) = fill {
            b = b.fill(f).hover(f.alpha(0.9));
        }
        b.child(text(label.to_owned()).font(UI_MED).color(ink))
    };
    let id = id.clone();
    let card = col()
        .key("delete-card")
        .w(380.0)
        .padding(20.0)
        .gap(6.0)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(8.0, 32.0, p.shadow_far)
        .child(text("Delete session").font(rune::TextStyle::new(15.0, 600.0).lh(24.0)).color(p.text))
        .child(text(format!("Delete session \"{name}\"?")).font(UI).color(p.muted))
        .child(
            row()
                .key("delete-footer")
                .gap(8.0)
                .mt(14.0)
                .child(spacer())
                .child(button("delete-cancel", "Cancel", p.text, None).on_press_tx(|s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.menus.confirm = None;
                }))
                .child(
                    button("delete-confirm", "Delete session", p.on_fill, Some(p.danger))
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.delete_session(&id, tx)),
                ),
        );
    vec![
        el().key("delete-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(p.dim)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.menus.confirm = None;
            }),
        el().key("delete-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

/// `lines` as the drawn menu draws them (`popup::surface`), item `hot` lit as if the pointer
/// were on it, for the storybook's stills.
pub fn preview(p: &Pal, key: &str, lines: &[Line], hot: Option<&str>) -> El {
    let lit = lines.iter().position(|l| matches!(l, Line::Item(i) if hot == Some(i.id)));
    crate::popup::surface(p, &crate::session_menu::menu(lines), lit).key(key.to_owned()).raise()
}
