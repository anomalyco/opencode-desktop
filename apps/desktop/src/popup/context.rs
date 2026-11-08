//! Right-clicking in the timeline: one menu at the pointer, made of what's under it. Selected
//! text in the message comes first (Copy, Quote), then an image's items (Copy Image, Save Image
//! As…, Open in Preview), then the message's own, the same items its chevron opens
//! (`rewind::message_items`).

use rune::{PointerPress, PopupAnchor, Tx};
use rune_theme_opencode::icons as ic;
use session_view::images::menu::{self as image_menu, MenuAt};

use crate::App;
use crate::popup::{Item, Menu};

const COPY_SELECTION: &str = "selection-copy";
const QUOTE_SELECTION: &str = "selection-quote";

/// The menu for a right-click on `at`, `None` when nothing there has items.
pub fn menu(s: &App, at: &MenuAt) -> Option<Menu> {
    let mut menu = Menu::new(opencode_i18n::t!("ui.message.moreActions")).min_width(160.0);
    let mut any = false;
    if let Some(selected) = at.message.as_deref().and_then(|m| crate::quote::selected_in(s, m)) {
        menu = menu
            .item(Item::new(COPY_SELECTION, opencode_i18n::t!("ui.message.copy")).icon(&ic::OUTLINE_COPY))
            .item(Item::new(QUOTE_SELECTION, opencode_i18n::t!("rune.message.quote")).icon(&crate::quote::OUTLINE_QUOTE).enabled(crate::quote::can_quote(s, &selected)));
        any = true;
    }
    if at.image.is_some() {
        if any {
            menu = menu.separator();
        }
        menu = menu.items(image_menu::items().into_iter().map(|(id, title)| Item::new(id, title).icon(image_icon(id))));
        any = true;
    }
    if let (Some(session), Some(message)) = (at.session.as_deref(), at.message.as_deref()) {
        let rest = if any { menu.clone().separator() } else { menu.clone() };
        if let Some(full) = crate::rewind::message_items(s, session, message, rest) {
            return Some(full);
        }
    }
    any.then_some(menu)
}

fn image_icon(id: &str) -> &'static rune::Icon {
    match id {
        image_menu::COPY => &ic::OUTLINE_COPY,
        image_menu::SAVE => &ic::DOWNLOAD,
        _ => &ic::SQUARE_ARROW_TOP_RIGHT,
    }
}

/// Opens [`menu`] at the pointer and runs what's picked.
pub fn open(s: &mut App, at: MenuAt, press: PointerPress, tx: &mut Tx) {
    show(s, at, press.into(), false, tx);
}

/// [`open`] at `anchor`, drawn on every platform when `drawn` (the storybook's still).
pub fn show(s: &mut App, at: MenuAt, anchor: PopupAnchor, drawn: bool, tx: &mut Tx) {
    let Some(menu) = menu(s, &at) else { return };
    let selected = at.message.as_deref().and_then(|m| crate::quote::selected_in(s, m));
    tx.instant();
    let pick: Pick = Box::new(move |s: &mut App, picked: Option<&str>, tx: &mut Tx| {
        let Some(id) = picked else { return };
        match id {
            COPY_SELECTION => {
                if let Some(text) = &selected {
                    tx.copy(text.clone());
                }
            }
            QUOTE_SELECTION => {
                if let Some(text) = &selected {
                    crate::quote::quote_selected(s, text, tx);
                }
            }
            _ => {
                if let Some(d) = &at.image
                    && image_menu::run(s, d, id, tx)
                {
                    return;
                }
                if let (Some(sid), Some(mid)) = (&at.session, &at.message) {
                    crate::rewind::run_message_item(s, sid, mid, id, tx);
                }
            }
        }
    });
    if drawn {
        crate::popup::open_drawn(s, menu, anchor, tx, pick);
    } else {
        crate::popup::open(s, menu, anchor, tx, pick);
    }
}

type Pick = Box<dyn FnOnce(&mut App, Option<&str>, &mut Tx)>;

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
