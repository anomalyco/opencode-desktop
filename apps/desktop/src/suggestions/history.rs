//! Shift+↑'s list of sent prompts over the composer (`editor.tsx` `ComposerEditorPopover` with
//! `history`): a search field pinned over whole rows, the highlighted one previewed in the
//! draft. Focus leaving it for the draft keeps the preview to edit (`popover.close`); leaving
//! it for anywhere else puts the draft back (`popover.dismiss`).

use rune::{Color, Cx, El, TextStyle, Tx, col, row, text};

use crate::App;
use crate::mentions::list::{boundary, max_height, suggest_search};
use crate::palette::{Elevate as _, Pal};
use crate::view::UI;

/// `palette-menu-row[data-size=compact]` (`palette-menu.css:40-67`): 28 tall, 8 in, 6 pt corners.
const ROW_H: f32 = 28.0;
const PAD: f32 = 6.0;
/// `palette-menu-title`: 13 at 530 on the 16 pt compact line, -0.04 tracking.
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
/// A shell entry's title: the mono face at 440.
const SHELL_TITLE: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(16.0).tracking(-0.04);

impl App {
    /// The sent prompts the history list's query finds, newest first, by id (`history:<n>`) and
    /// title (`interaction.ts` `suggestions`: the label and the description, lowercased).
    pub fn history_rows(&self, query: &str) -> Vec<(String, String)> {
        let query = query.trim().to_lowercase();
        self.prompts
            .entries(self.add.shell())
            .iter()
            .enumerate()
            .filter(|(_, e)| e.text.to_lowercase().contains(&query))
            .map(|(i, e)| (format!("history:{i}"), e.text.clone()))
            .collect()
    }
}

pub fn view(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    let crate::suggestions::machine::Popover::History { list, .. } = &s.suggest.state else { return None };
    let width = cx.layout_size("composer").map_or(0.0, |[w, _]| w);
    let rows = s.history_rows(&list.query);
    let active = list.active.id();
    let font = if s.add.shell() { SHELL_TITLE } else { TITLE };
    let lit = if p.dark { Color::WHITE.alpha(0.10) } else { p.hover };
    let mut items = col().key("history-list").role("listbox").label_static(opencode_i18n::t!("ui.promptInput.history")).w_full();
    if rows.is_empty() {
        items = items.child(row().key("empty").h(28.0).px(8.0).center_y().child(text(opencode_i18n::t!("ui.promptInput.noMatchingItems")).font(UI).color(p.muted)));
    }
    for (id, title) in &rows {
        let (point, pick) = (id.clone(), id.clone());
        let title = if title.is_empty() { opencode_i18n::t!("ui.promptInput.history.contextOnly").to_owned() } else { title.replace('\n', " ") };
        items = items.child(
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
                .fill(if active == Some(id.as_str()) { lit } else { Color::TRANSPARENT })
                .on_pointer_move_local(move |s: &mut App, _, tx: &mut Tx| s.suggest_pointed(&point, tx))
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.suggest_pick(&pick, tx))
                .child(text(title).font(font).color(p.text).truncate().grow(1.0).shrink().min_w(0.0)),
        );
    }
    let limit = max_height(boundary(cx).unwrap_or(2.0 * 320.0), true, true);
    let list_h = (rows.len().max(1) as f32 * ROW_H).min(limit - 2.0 * PAD - 28.0);
    Some(
        col()
            .elevate(p.raised)
            .key("history")
            .above("composer", 8.0)
            .w(width)
            .padding(PAD)
            .radius(10.0)
            .fill(p.bg_base)
            .clip_soft(0.0)
            .transition(rune::Transition::none())
            .preserve_focus_on_press(true)
            .on_press(|_: &mut App| {})
            .child(suggest_search(p, &s.suggest.search, opencode_i18n::t!("ui.promptInput.history"), opencode_i18n::t!("ui.promptInput.history.search")))
            // `MenuScroll` (`editor.tsx:1314`).
            .child(session_view::scroller::scroller("history-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(p, col().w_full().h(list_h), items)),
    )
}
