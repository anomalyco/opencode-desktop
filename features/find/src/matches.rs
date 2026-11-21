//! Match highlight queries on [`Find`], long-prompt fold expansion, and active-match reveal.

use std::ops::Range;

use opencode_kit::model::Message;
use rune::{TextHighlight, Tx};
use session_view::palette::Pal;
pub use session_view::style::{ACTIVE_KEY, markdown_mark, text_mark};

use crate::scan::Answer;
use crate::transcript_index::{Results, RowHit, row_hash};
use crate::{Find, FindHost};

impl Find {
    pub(crate) fn results(&self) -> Option<&Results> {
        match &self.open()?.answer {
            Answer::None | Answer::Waiting { .. } => None,
            Answer::Partial { results, .. } | Answer::Complete { results, .. } => Some(results),
        }
    }

    /// The query the matches are for.
    pub(crate) fn answered(&self) -> Option<&str> {
        match &self.open()?.answer {
            Answer::None => None,
            Answer::Waiting { query, .. } | Answer::Partial { query, .. } | Answer::Complete { query, .. } => Some(query),
        }
    }

    /// The lit match: its row and its place in the row.
    pub(crate) fn lit(&self) -> Option<(&RowHit, usize)> {
        self.results()?.locate(self.active_index())
    }

    /// The lit match's place in `row_key`'s row, when it is there.
    fn active_in(&self, row_key: &str) -> Option<usize> {
        self.lit().filter(|(h, _)| &*h.key == row_key).map(|(_, local)| local)
    }

    /// Hash included in `RowSpec::build`'s `rune::memo` key: `0` when Find is closed or `row_key`
    /// has no matches, so non-matching rows never rebuild on keystrokes or stepping.
    pub fn row_version(&self, row_key: &str) -> u64 {
        let Some(hit) = self.results().and_then(|r| r.get(row_key)) else { return 0 };
        match self.active_in(row_key) {
            Some(local) => row_hash(&hit.ranges, Some(local)),
            None => hit.version,
        }
    }

    /// Highlights for a user bubble (`text(shown)`).
    pub fn text_highlights(&self, p: &Pal, row_key: &str) -> Vec<TextHighlight> {
        let Some(row) = self.results().and_then(|r| r.get(row_key)) else { return Vec::new() };
        let active = self.active_in(row_key);
        row.ranges
            .iter()
            .enumerate()
            .filter_map(|(i, rs)| Some(text_mark(p, rs.first()?.clone(), i, active == Some(i))))
            .collect()
    }

    /// Highlights for an assistant `Markdown` block (`Part::Text`).
    pub fn markdown_highlights(&self, p: &Pal, row_key: &str) -> Vec<(&[Range<usize>], rune_kit::Highlight)> {
        let Some(row) = self.results().and_then(|r| r.get(row_key)) else { return Vec::new() };
        let active = self.active_in(row_key);
        row.ranges.iter().enumerate().map(|(i, rs)| (rs.as_slice(), markdown_mark(p, i, active == Some(i)))).collect()
    }

    /// A row's matches (source byte ranges of its text) and which of them is active, for a
    /// view that paints them itself (`genui`).
    #[allow(clippy::type_complexity)]
    pub fn row_hits(&self, row_key: &str) -> Option<(&[Vec<Range<usize>>], Option<usize>)> {
        let hit = self.results()?.get(row_key)?;
        Some((hit.ranges.as_slice(), self.active_in(row_key)))
    }
}

/// Opens a folded long user prompt when the lit match falls in or past its 40 pt fading
/// region (`search-controller.ts`: `rect.bottom > fold.bottom - FOLD_FADE`).
pub fn unfold_if_hidden(s: &mut dyn FindHost, sid: &str, row_key: &str, end_byte: usize) -> bool {
    if !s.fold_user_messages() {
        return false;
    }
    let Some(id) = row_key.strip_prefix("u-") else { return false };
    let fold_key = session_view::fold::key(id);
    if s.expanded().contains(&fold_key) {
        return false;
    }
    let Some(body) = s.transcript(sid).and_then(|t| {
        t.messages.iter().find_map(|m| match m {
            Message::User { id: mid, text } if mid == id => Some(text.clone()),
            _ => None,
        })
    }) else {
        return false;
    };
    let shown = s.bubble_text(id, &body);
    let quotes = !opencode_kit::model::user_comments(id).is_empty() && !body.trim().is_empty();
    let wrap_w = session_view::fold::text_width(session_view::fold::viewport_width(), quotes);
    if session_view::fold::lines(&shown, wrap_w) <= session_view::fold::OVER {
        return false;
    }
    let end = shown.ceil_char_boundary(end_byte.min(shown.len()));
    let prefix = &shown[..end];
    let (_, measured_h) = session_view::fold::measure(prefix, wrap_w);
    let hard_h = prefix.split('\n').count() as f32 * session_view::style::BUBBLE.line_px();
    let clear = session_view::fold::SHOW as f32 * session_view::style::BUBBLE.line_px() - 40.0;
    if measured_h.max(hard_h) <= clear {
        return false;
    }
    drop(shown);
    s.expanded_mut().insert(fold_key);
    true
}

/// Scrolls the timeline to bring the lit match into view, unfolding its user bubble if needed.
pub fn seek_active(s: &mut dyn FindHost, tx: &mut Tx) {
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
    let Some((row_key, end_byte)) = s
        .find()
        .lit()
        .map(|(h, local)| (h.key.to_string(), h.ranges[local].last().map_or(0, |r| r.end)))
    else {
        return;
    };
    if unfold_if_hidden(s, &sid, &row_key, end_byte) {
        tx.instant();
    }
    let scroll_key = format!("scroll-{sid}");
    // Mount the match's row, then land the match where Desktop's `probeSeek` puts it
    // (`search-controller.ts:305-308`, scrolled with `behavior: "auto"`): centred in the full
    // scroll viewport down to the page foot. Official places `[data-session-title]` outside
    // `<ScrollView>` (`virtualizer.tsx:1182-1190`), so `root.querySelector("[data-session-title]")`
    // returns null (`inset = 0`).
    tx.scroll_center_in_instant(&scroll_key, &row_key);
    tx.scroll_center_in_instant(&scroll_key, ACTIVE_KEY);
    if let (Some([_, top, _, _]), Some([_, page_y, _, page_h])) = (s.timeline_view_rect(), s.timeline_page_rect()) {
        // `scroll_key_to` places the top of Rune's highlight *line box*. Blink instead places
        // the text range, already inset by half the leading. Centre the line, not an 18 pt
        // glyph box with its top mistaken for the line's (3–4 device pixels low at 1.875x).
        let line = if row_key.starts_with("u-") { session_view::style::BUBBLE } else { session_view::style::MD };
        tx.scroll_key_to(ACTIVE_KEY, (page_y + page_h - top - line.line_px()) / 2.0 + 0.25);
    }
}
