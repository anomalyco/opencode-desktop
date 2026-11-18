//! What the session menus and rename draw: the title field, Delete's confirmation, and a drawn
//! copy of a menu for the storybook and stills (native menus never draw headless).

use opencode_i18n::t;
use opencode_ui::popup::{self, PopupHost};
use opencode_ui::widgets::{BODY, DialogSize, Fit, button, dialog_window, sized};
use rune::{Color, Cx, Direction, El, TextStyle, Tx, col, el, row, spacer, text};
use rune_kit::ButtonVariant;
use opencode_ui::palette::flat;
use opencode_ui::palette::{Elevate as _, Pal, kit};

use crate::host::SessionMenuHost;
use crate::{Line, Spot, Target};

/// `dialog-title`: 15 on 20 at 530, -0.13 tracking.
pub const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);

/// Room past a hugging title field's text for Rune's 1.5 pt caret, with rounding slack.
const CARET_ROOM: f32 = 2.0;

fn field_shell(host: &dyn SessionMenuHost, p: &Pal, target: &Target, style: TextStyle) -> Option<El> {
    if !host.menus().renames(target) {
        return None;
    }
    let input = opencode_ui::style::field(&host.menus().edit, style)
        .key("rename")
        .spellcheck(host.spellcheck())
        .single_line()
        .dir(Direction::Auto)
        .truncate_fade()
        .h(style.line_px());
    // Outside a tab the field hugs its text, but a focused single-line input reserves 1.5 pt for
    // the caret and scrolls the text left by that much when the caret is at the end, clipping the
    // first glyph. Room past the text (and any composition) keeps it in view; it still shrinks
    // and scrolls when the title is longer than the room.
    let input = match target.spot {
        Spot::Tab => input.grow(1.0),
        _ => input.w(rune::text::measure(&host.menus().edit.display().0, &[], style, None).0 + CARET_ROOM),
    };
    Some(input.color(p.text).caret(p.text, p.selection).selection_style(p.field_selection))
}

fn submit_inline_rename(host: &mut dyn SessionMenuHost, tx: &mut Tx) {
    tx.instant();
    crate::finish_rename(host, true);
}

/// `s`'s title field, in `style`, when `target` is the one being renamed. Outside a tab it hugs
/// its text, so what follows stays put and moves only as the title gets longer; one too long for
/// the room scrolls inside it and fades at the edge. It stands one line of `style` tall, so its
/// selection is as tall as the line, as a field's is in Chromium.
pub fn field<H: SessionMenuHost + 'static>(s: &H, p: &Pal, target: &Target, style: TextStyle) -> Option<El> {
    field_shell(s, p, target, style).map(|input| {
        input
            .on_edit(|s: &mut H| &mut s.menus_mut().edit)
            .on_input_key(|s: &mut H, ev, tx: &mut Tx| crate::key(s, ev, tx))
            .on_submit(|s: &mut H, tx: &mut Tx| submit_inline_rename(s, tx))
    })
}

/// Delete's confirmation (`home/sessions/controller.tsx:502-522`): Desktop's `<Dialog fit>` over
/// its scrim, the title group in its header and a ghost Cancel beside the danger button in its
/// footer.
pub fn confirm<H: SessionMenuHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    let Some(asked) = s.menus().asked() else {
        return vec![];
    };
    let name = asked.title.clone();
    let k = kit(p);
    // Delete closes the dialog at once (the delete waits for its undo toast), so its buttons are
    // never off waiting for the server, as Desktop's are.
    let cancel = button(t!("common.cancel"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut H, tx: &mut Tx| crate::cancel_delete(s, tx))
        .build(k)
        .key("delete-cancel")
        .autofocus();
    // `danger` (`button.css:102-106`): the neutral surface and its elevation, in the danger ink.
    // Dark's neutral fill is `alpha-light-6`, which Chromium lays over the dialog's `layer-01`,
    // where the theme's token is flattened over `bg-base`. Opaque, as the elevation's ring and
    // drop would show through a translucent fill.
    let delete = button(t!("session.delete.button"), ButtonVariant::Danger)
        .on_press_tx(|s: &mut H, tx: &mut Tx| crate::delete_session(s, tx))
        .build(k)
        .key("delete-confirm");
    let delete = if p.dark && p.theme == 0 { delete.fill(flat(Color::WHITE.alpha(0.06), p.layer1)) } else { delete };
    // `<Dialog fit>` (`shell/titlebar/project-session-row.tsx`): the normal size, 6 pt corners on `layer-01` with the
    // overlay elevation; a 16 pt header with the title (15/20 at 530) 4 over the description
    // (13/20 at 440, muted), and a 16 pt footer, its buttons 8 apart at its end.
    let card = col()
        .key("delete-card")
        .role("dialog")
        .label_static(t!("session.delete.title"))
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        // Rune's modal: Tab stays inside, Escape and a press outside cancel, Cancel opens focused
        // (`autofocus` in `controller.tsx`) and closing hands focus back.
        .modal_tx(true, |s: &mut H, tx: &mut Tx| crate::cancel_delete(s, tx))
        .child(
            col()
                .key("delete-header")
                .w_full()
                .padding(16.0)
                .gap(4.0)
                .child(text(t!("session.delete.title")).font(TITLE).color(p.text))
                .child(text(t!("session.delete.confirm", name = name)).font(BODY).color(p.muted).wrap().w_full()),
        )
        .child(row().key("delete-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).child(delete));
    let card = sized(card, DialogSize::Normal, Fit::Content, dialog_window(cx, s.zoom()));
    vec![
        el().key("delete-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim),
        el().key("delete-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

/// `lines` as the drawn menu draws them through [`SessionMenuHost::popup_surface`], item `hot`
/// lit as if the pointer were on it, for the storybook's stills.
pub fn preview(host: &dyn SessionMenuHost, p: &Pal, key: &str, lines: &[Line], hot: Option<&str>) -> El {
    let lit = lines.iter().position(|l| matches!(l, Line::Item(i) if hot == Some(i.id)));
    host.popup_surface(p, &crate::menu(lines), lit).key(key.to_owned()).raise()
}

/// [`preview`] specialized directly to any [`PopupHost`] without needing a [`SessionMenuHost`] reference.
pub fn preview_for<H: PopupHost + 'static>(p: &Pal, key: &str, lines: &[Line], hot: Option<&str>) -> El {
    let lit = lines.iter().position(|l| matches!(l, Line::Item(i) if hot == Some(i.id)));
    popup::surface::<H>(p, &crate::menu(lines), lit).key(key.to_owned()).raise()
}
