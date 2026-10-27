//! What the session menus and rename draw: the title field, Delete's confirmation, and a drawn
//! copy of a menu for the storybook and stills (native menus never draw headless).

use rune::{Color, El, Tx, col, el, row, spacer, text, text_input};

use crate::palette::Pal;
use crate::session_menu::{Item, Line, Spot, Target};
use crate::view::{UI, UI_MED};
use crate::App;

/// `s`'s title field, in `style`, when `target` is the one being renamed.
pub fn field(s: &App, p: &Pal, target: &Target, style: rune::TextStyle) -> Option<El> {
    if s.menus.rename.as_ref() != Some(target) {
        return None;
    }
    let width = (s.menus.edit.text.chars().count() as f32 * 7.4 + 16.0).clamp(96.0, 480.0);
    let input = text_input(&s.menus.edit, style).key("rename").single_line().min_w(0.0).shrink().h(16.0);
    let input = if target.spot == Spot::Tab { input.grow(1.0) } else { input.w(width) };
    Some(
        input
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
                    button("delete-confirm", "Delete session", Color::WHITE, Some(p.danger))
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.delete_session(&id, tx)),
                ),
        );
    vec![
        el().key("delete-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(Color::BLACK.alpha(if p.dark { 0.5 } else { 0.25 }))
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.menus.confirm = None;
            }),
        el().key("delete-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

/// `lines` as a menu: rounded, raised, 13 pt; item `hot` lit as if the pointer were on it.
pub fn preview(p: &Pal, key: &str, lines: &[Line], hot: Option<&str>) -> El {
    let mut c = col().key(key.to_owned()).w(188.0).padding(5.0).radius(8.0).fill(p.bg_base).border(0.5, p.ring).shadow(10.0, 28.0, p.shadow).raise();
    for (n, l) in lines.iter().enumerate() {
        c = c.child(match l {
            Line::Item(i) => entry(p, i, hot == Some(i.id)),
            Line::Sep => el().key(format!("sep-{n}")).w_full().h(1.0).my(5.0).px(10.0).child(el().key("line").w_full().h(1.0).fill(p.ring)),
        });
    }
    c
}

fn entry(p: &Pal, i: &Item, hot: bool) -> El {
    let ink = if hot { Color::WHITE } else if i.enabled { p.text } else { p.faint };
    let mut r = row().key(format!("item-{}", i.id)).w_full().h(22.0).px(10.0).radius(4.0).center_y();
    if hot {
        r = r.fill(p.bg_accent);
    }
    r.child(text(i.title).font(UI).color(ink))
}
