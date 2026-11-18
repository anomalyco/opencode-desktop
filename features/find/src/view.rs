//! Session header Find bar (`TimelineSearchBar`) and native Edit menu items.
//!
//! Layout and chrome live in non-generic `&dyn FindHost` builders; thin `H: FindHost + 'static`
//! entrypoints attach only the `rune::El` / `MenuItem` callbacks delegating to non-generic logic.

use opencode_ui::keymap::Bind;
use rune::{El, MenuItem, TextStyle, Transition, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;
use session_view::palette::Pal;
use session_view::style::{INSTANT, UI};

use crate::FindHost;

const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).features("tnum");

fn input_shell(s: &dyn FindHost, p: &Pal) -> El {
    session_view::style::field(&s.find().input, UI)
        .key("find-input")
        .spellcheck(s.spellcheck())
        .single_line()
        .truncate_fade()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h(20.0)
        .color(p.text)
        .placeholder(opencode_i18n::t!("session.search.placeholder"), p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
}

fn close_button_shell(p: &Pal) -> El {
    row()
        .key("find-close")
        .role("button")
        // Desktop names it after the field and gives it no tooltip (`search-bar.tsx`).
        .label_static(opencode_i18n::t!("session.search.placeholder"))
        .size(20.0, 20.0)
        .center()
        .radius(2.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted))
}

fn build_bar(s: &dyn FindHost, p: &Pal, input: El, close_btn: El) -> El {
    let find = s.find();
    // `{activeDisplay}/{count}` while the field holds anything, a space included (`showCount`).
    let total = find.count();
    let count = (!find.input.text.is_empty()).then(|| {
        let shown = if total > 0 { find.active_index() + 1 } else { 0 };
        // Until the worker has matched the rows off screen, the total isn't known.
        let total = if find.counting() { "…".to_owned() } else { total.to_string() };
        text(format!("{shown}/{total}")).key("find-count").font(COUNT).color(p.muted)
    });
    // `text-input-v2`: its input elevation at rest, the focus outline (and no shadow) while focused.
    let bar = row().key("find-bar");
    let bar = if find.input.focused { bar.border(1.0, p.v2.border_focus) } else { bar.box_shadow(Pal::ring_out(p.border)) };
    bar.transition(Transition::none())
        .h(28.0)
        .w(200.0)
        .pl(8.0)
        .pr(4.0)
        // The value (icon and field, 6 apart), the count and the close button, 8 apart.
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_base)
        .child(
            row()
                .key("find-value")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(6.0)
                .center_y()
                .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
                .child(input),
        )
        .children(count)
        .child(close_btn)
}

/// The session header's Find bar (`TimelineSearchBar`).
pub fn bar<H: FindHost + 'static>(s: &H, p: &Pal) -> Option<El> {
    if !s.find().is_open() {
        return None;
    }
    let input = input_shell(s, p)
        .on_edit(|s: &mut H| &mut s.find_mut().input)
        .on_input_change(|s: &mut H, tx: &mut Tx| crate::typed(s, tx))
        .on_input_key(|s: &mut H, ev, tx| crate::input_key(s, ev, tx));
    let close = close_button_shell(p).on_press_tx(|s: &mut H, tx: &mut Tx| crate::close(s, tx));
    Some(build_bar(s, p, input, close))
}

fn keyed<H: 'static>(item: MenuItem<H>, bind: Bind) -> MenuItem<H> {
    match bind.shortcut() {
        Some(keys) => item.key_equivalent(keys),
        None => item,
    }
}

/// Native macOS Edit menu items (`keys_first` so focused fields and `on_key` see them first),
/// with the keys their commands have now: a group after Select All, ahead of the items AppKit
/// adds to the menu's end (AutoFill, Dictation, Emoji & Symbols).
pub fn menu_items<H: FindHost + 'static>() -> Vec<MenuItem<H>> {
    let item = |title, id, key: Bind, f: fn(&mut H, &mut Tx)| {
        keyed(MenuItem::new(title, f).id(id).menu("Edit").after("selectAll:"), key).keys_first()
    };
    [
        MenuItem::separator().menu("Edit").after("selectAll:"),
        item(opencode_i18n::t!("rune.menu.edit.find"), "Edit/Find", Bind::Find, |s, tx| crate::open_from_menu(s, tx)),
        item(opencode_i18n::t!("rune.menu.edit.findNext"), "Edit/FindNext", Bind::FindNext, |s, tx| crate::step(s, 1, tx)),
        item(opencode_i18n::t!("rune.menu.edit.findPrevious"), "Edit/FindPrevious", Bind::FindPrev, |s, tx| crate::step(s, -1, tx)),
    ]
    .into()
}
