//! Shift+↑'s list of sent prompts over the composer (`editor.tsx` `ComposerEditorPopover` with
//! `history`): a search field pinned over whole rows, the highlighted one previewed in the
//! draft. Focus leaving it for the draft keeps the preview to edit (`popover.close`); leaving
//! it for anywhere else puts the draft back (`popover.dismiss`).

use rune::{Color, Cx, El, TextStyle, Tx, col, row, text};
use session_view::composer::composer_fill;
use session_view::palette::{Elevate as _, Pal};
use session_view::scroller::{Bar, scroller};
use session_view::style::UI;

use super::SuggestionsHost;
use super::machine::Popover;
use super::popover::{boundary, max_height, suggest_search};

/// `palette-menu-row[data-size=compact]` (`palette-menu.css:40-67`): 28 tall, 8 in, 6 pt corners.
const ROW_H: f32 = 28.0;
const PAD: f32 = 6.0;
/// `palette-menu-title`: 13 at 530 on the 16 pt compact line, -0.04 tracking.
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// A shell entry's title: the mono face at 440.
const SHELL_TITLE: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(16.0).tracking(-0.04);

/// The sent prompts the history list's query finds, newest first, by id (`history:<n>`) and
/// title (`interaction.ts` `suggestions`: the label and the description, lowercased).
pub fn history_rows(host: &dyn SuggestionsHost, query: &str) -> Vec<(String, String)> {
    let query = query.trim().to_lowercase();
    host.prompts()
        .entries(host.shell_mode())
        .iter()
        .enumerate()
        .filter(|(_, e)| e.text.to_lowercase().contains(&query))
        .map(|(i, e)| (format!("history:{i}"), e.text.clone()))
        .collect()
}

struct PreparedHistory {
    rows: Vec<(String, String)>,
    active: Option<String>,
    font: TextStyle,
    lit: Color,
    list_h: f32,
}

fn prepare_history_view(host: &dyn SuggestionsHost, p: &Pal, boundary: Option<f32>) -> Option<PreparedHistory> {
    let Popover::History { list, .. } = &host.suggest().state else { return None };
    let rows = history_rows(host, &list.query);
    let active = list.active.id().map(str::to_owned);
    let font = if host.shell_mode() { SHELL_TITLE } else { TITLE };
    let lit = if p.dark { Color::WHITE.alpha(0.10) } else { p.hover };
    let limit = max_height(boundary.unwrap_or(2.0 * 320.0), true, true);
    let list_h = (rows.len().max(1) as f32 * ROW_H).min(limit - 2.0 * PAD - 28.0);
    Some(PreparedHistory { rows, active, font, lit, list_h })
}

fn on_pointed(host: &mut dyn SuggestionsHost, id: &str, tx: &mut Tx) {
    super::suggest_pointed(host, id, tx);
}

fn on_pick(host: &mut dyn SuggestionsHost, id: &str, tx: &mut Tx) {
    super::suggest_pick(host, id, tx);
}

fn row_shell(p: &Pal, id: &str, title: String, font: TextStyle, is_active: bool, lit: Color) -> El {
    row()
        .key(format!("history-row-{id}"))
        .role("option")
        .label(title.clone())
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .fill(if is_active { lit } else { Color::TRANSPARENT })
        .child(text(title).font(font).color(p.text).truncate().grow(1.0).shrink().min_w(0.0))
}

fn frame_shell(p: &Pal, width: f32, list_h: f32, search: El, row_els: Vec<El>) -> El {
    let mut items = col().key("history-list").role("listbox").label_static(opencode_i18n::t!("ui.promptInput.history")).w_full();
    if row_els.is_empty() {
        items = items.child(
            row()
                .key("empty")
                .h(28.0)
                .px(8.0)
                .center_y()
                .child(text(opencode_i18n::t!("ui.promptInput.noMatchingItems")).font(UI).color(p.muted)),
        );
    } else {
        items = items.children(row_els);
    }
    col()
        .elevate(p.raised)
        .key("history")
        .above("composer", 8.0)
        .w(width)
        .padding(PAD)
        .radius(10.0)
        .fill(composer_fill(p))
        .clip_soft(0.0)
        .transition(rune::Transition::none())
        .preserve_focus_on_press(true)
        .child(search)
        // `MenuScroll` (`editor.tsx:1314`).
        .child(scroller("history-scroll").bar(Bar::MenuScroll).build::<()>(p, col().w_full().h(list_h), items))
}

pub fn view<H: SuggestionsHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Option<El> {
    if !s.suggest().state.is_history() {
        return None;
    }
    let width = cx.layout_size("composer").map_or(0.0, |[w, _]| w);
    let prepared = prepare_history_view(s, p, boundary(cx))?;
    let row_els = prepared
        .rows
        .iter()
        .map(|(id, title)| {
            let (point, pick) = (id.clone(), id.clone());
            let title = if title.is_empty() {
                opencode_i18n::t!("ui.promptInput.history.contextOnly").to_owned()
            } else {
                title.replace('\n', " ")
            };
            let is_active = prepared.active.as_deref() == Some(id.as_str());
            row_shell(p, id, title, prepared.font, is_active, prepared.lit)
                .on_pointer_move_local(move |s: &mut H, _, tx: &mut Tx| on_pointed(s, &point, tx))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| on_pick(s, &pick, tx))
        })
        .collect();
    let search = suggest_search::<H>(
        p,
        &s.suggest().search,
        opencode_i18n::t!("ui.promptInput.history"),
        opencode_i18n::t!("ui.promptInput.history.search"),
    );
    Some(frame_shell(p, width, prepared.list_h, search, row_els).on_press(|_: &mut H| {}))
}
