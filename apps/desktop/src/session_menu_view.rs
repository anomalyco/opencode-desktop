//! What the session menus and rename draw: the title field, Delete's confirmation, and a drawn
//! copy of a menu for the storybook and stills (native menus never draw headless).

use rune::{Color, El, TextStyle, Tx, col, el, row, spacer, text, text_input};
use rune_kit::ButtonVariant;

use crate::palette::{Elevate as _, Pal};
use crate::session_menu::{Line, Spot, Target};
use crate::App;
use opencode_i18n::t;

/// `dialog-title`: 15 on 20 at 530, -0.13 tracking.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);

/// `s`'s title field, in `style`, when `target` is the one being renamed. Outside a tab it hugs
/// its text, so what follows stays put and moves only as the title gets longer; one too long for
/// the room scrolls inside it and fades at the edge. It stands one line of `style` tall, so its
/// selection is as tall as the line, as a field's is in Chromium.
pub fn field(s: &App, p: &Pal, target: &Target, style: rune::TextStyle) -> Option<El> {
    if !s.menus.renames(target) {
        return None;
    }
    let input = text_input(&s.menus.edit, style).key("rename").spellcheck(s.settings.spellcheck).single_line().dir(rune::Direction::Auto).truncate_fade().h(style.line_px());
    let input = if target.spot == Spot::Tab { input.grow(1.0) } else { input };
    Some(
        input
            .color(p.text)
            .caret(p.text, p.selection).selection_style(p.field_selection)
            .on_edit(|s: &mut App| &mut s.menus.edit)
            .on_input_key(|s: &mut App, ev, tx: &mut Tx| crate::session_menu::key(s, ev, tx))
            .on_submit(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.finish_rename(true);
            }),
    )
}

/// Delete's confirmation (`home/sessions/controller.tsx:502-522`): Desktop's `<Dialog fit>` over
/// its scrim, the title group in its header and a ghost Cancel beside the danger button in its
/// footer.
pub fn confirm(s: &App, p: &Pal) -> Vec<El> {
    let Some(asked) = s.menus.asked() else { return vec![] };
    let name = asked.title.clone();
    let kit = crate::palette::kit(p);
    // Delete closes the dialog at once (the delete waits for its undo toast), so its buttons are
    // never off waiting for the server, as Desktop's are.
    let cancel = crate::widgets::button(t!("common.cancel"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.cancel_delete(tx))
        .build(kit)
        .key("delete-cancel")
        .autofocus();
    // `danger` (`button.css:102-106`): the neutral surface and its elevation, in the danger ink.
    // Dark's neutral fill is `alpha-light-6`, which Chromium lays over the dialog's `layer-01`,
    // where the theme's token is flattened over `bg-base`. Opaque, as the elevation's ring and
    // drop would show through a translucent fill.
    let delete = crate::widgets::button(t!("session.delete.button"), ButtonVariant::Danger)
        .on_press_tx(|s: &mut App, tx: &mut Tx| s.delete_session(tx))
        .build(kit)
        .key("delete-confirm");
    let delete = if p.dark && p.theme == 0 { delete.fill(session_view::dock::flat(Color::WHITE.alpha(0.06), p.layer1)) } else { delete };
    // `dialog.css:19-28, 48-106, 132-141, 161-168`: 480 wide, 6 pt corners on `layer-01` with the
    // overlay elevation; a 16 pt header with the title (15/20 at 530) 4 over the description
    // (13/20 at 440, muted), and a 16 pt footer, its buttons 8 apart at its end.
    let card = col()
        .key("delete-card")
        .role("dialog")
        .label(t!("session.delete.title"))
        .w(480.0)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        // Rune's modal: Tab stays inside, Escape and a press outside cancel, Cancel opens focused
        // (`autofocus` in `controller.tsx`) and closing hands focus back.
        .modal_tx(true, |s: &mut App, tx: &mut Tx| s.cancel_delete(tx))
        .child(
            col()
                .key("delete-header")
                .w_full()
                .padding(16.0)
                .gap(4.0)
                .child(text(t!("session.delete.title")).font(TITLE).color(p.text))
                .child(text(t!("session.delete.confirm", name = name)).font(crate::widgets::BODY).color(p.muted).wrap().w_full()),
        )
        .child(row().key("delete-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).child(delete));
    vec![
        el().key("delete-scrim")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(p.scrim),
        el().key("delete-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

/// `lines` as the drawn menu draws them (`popup::surface`), item `hot` lit as if the pointer
/// were on it, for the storybook's stills.
pub fn preview(p: &Pal, key: &str, lines: &[Line], hot: Option<&str>) -> El {
    let lit = lines.iter().position(|l| matches!(l, Line::Item(i) if hot == Some(i.id)));
    crate::popup::surface(p, &crate::session_menu::menu(lines), lit).key(key.to_owned()).raise()
}
