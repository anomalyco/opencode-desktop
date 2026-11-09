//! Find in a session (⌘F): Desktop's `TimelineSearchBar` and `createTimelineSearchController`
//! (`session/timeline/search-bar.tsx`, `search-controller.ts`).
//!
//! Matches are indexed across the whole visible transcript (mounted or not), cached per message
//! so streaming replies only re-scan the changed part, and hashed per row so `rune::memo` only
//! rebuilds mounted rows whose highlights or active match actually changed.

use crate::pickers::PickerHost as _;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use opencode_kit::model::{Assistant, Message, Part, Transcript};
use rune::{El, Key, KeyEvent, MenuItem, TextEdit, TextHighlight, TextStyle, Transition, Tx, icon, row, text, text_input};
use rune_theme_opencode::icons as ic;

use crate::keymap::Bind;
use crate::palette::Pal;
use crate::view::{INSTANT, UI};
use crate::{App, Route};

const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).features("tnum");

#[derive(Clone, Default)]
struct Saved {
    open: bool,
    query: String,
    active: usize,
    restore_composer: bool,
}

#[derive(Clone, Debug)]
struct Hit {
    row_key: String,
    local: usize,
    end_byte: usize,
}

#[derive(Clone, Default)]
struct RowHits {
    ranges: Vec<Vec<Range<usize>>>,
    active: Option<usize>,
    version: u64,
}

#[derive(Clone)]
struct CachedPart {
    len: usize,
    head_tail: u64,
    ranges: Vec<Vec<Range<usize>>>,
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub input: TextEdit,
    active: usize,
    restore_composer: bool,
    session: String,
    saved: HashMap<String, Saved>,

    /// Cached index state for the active session and query.
    indexed_session: String,
    indexed_query: String,
    indexed_fp: u64,
    parts: HashMap<(String, usize), CachedPart>,
    matches: Vec<Hit>,
    rows: HashMap<String, RowHits>,
}

impl Find {
    pub fn query(&self) -> &str {
        self.input.text.trim()
    }

    pub fn count(&self) -> usize {
        if self.open { self.matches.len() } else { 0 }
    }

    pub fn active_index(&self) -> usize {
        if self.matches.is_empty() { 0 } else { self.active.min(self.matches.len() - 1) }
    }

    /// Hash included in `RowSpec::build`'s `rune::memo` key: `0` when Find is closed or `row_key`
    /// has no matches, so non-matching rows never rebuild on keystrokes or stepping.
    pub fn row_version(&self, row_key: &str) -> u64 {
        if !self.open {
            return 0;
        }
        self.rows.get(row_key).map_or(0, |r| r.version)
    }

    /// Highlights for a user bubble (`text(shown)`).
    pub fn text_highlights(&self, p: &Pal, row_key: &str) -> Vec<TextHighlight> {
        let Some(row) = self.rows.get(row_key).filter(|_| self.open) else { return Vec::new() };
        row.ranges
            .iter()
            .enumerate()
            .filter_map(|(i, rs)| Some(text_mark(p, rs.first()?.clone(), i, row.active == Some(i))))
            .collect()
    }

    /// Highlights for an assistant `Markdown` block (`Part::Text`).
    pub fn markdown_highlights<'a>(&'a self, p: &Pal, row_key: &str) -> Vec<(&'a [Range<usize>], rune_kit::Highlight)> {
        let Some(row) = self.rows.get(row_key).filter(|_| self.open) else { return Vec::new() };
        row.ranges.iter().enumerate().map(|(i, rs)| (rs.as_slice(), markdown_mark(p, i, row.active == Some(i)))).collect()
    }

    /// A row's matches (source byte ranges of its text) and which of them is active, for a
    /// view that paints them itself (`genui`).
    pub fn row_hits(&self, row_key: &str) -> Option<(&[Vec<Range<usize>>], Option<usize>)> {
        self.rows.get(row_key).filter(|_| self.open).map(|r| (r.ranges.as_slice(), r.active))
    }
}

pub(crate) use session_view::style::{ACTIVE_KEY, markdown_mark, text_mark};

fn str_sig(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h = DefaultHasher::new();
    b.len().hash(&mut h);
    b[..b.len().min(32)].hash(&mut h);
    b[b.len().saturating_sub(32)..].hash(&mut h);
    h.finish()
}

fn transcript_fp(t: &Transcript, shown: usize) -> u64 {
    let mut h = DefaultHasher::new();
    shown.hash(&mut h);
    for m in &t.messages[..shown] {
        match m {
            Message::User { id, text } => (0u8, id, text.len(), str_sig(text)).hash(&mut h),
            Message::Assistant(a) => {
                (1u8, &a.id, a.parts.len()).hash(&mut h);
                for p in &a.parts {
                    if let Part::Text { text } = p {
                        (text.len(), str_sig(text)).hash(&mut h);
                    }
                }
            }
            _ => {}
        }
    }
    h.finish()
}

fn row_hash(ranges: &[Vec<Range<usize>>], active: Option<usize>) -> u64 {
    let mut h = DefaultHasher::new();
    ranges.hash(&mut h);
    active.hash(&mut h);
    h.finish().max(1)
}

/// Case-insensitive non-overlapping matches over plain UTF-8 text, preserving char boundaries
/// even when Unicode lowercasing changes byte length (`'İ'`, `'ẞ'`, `'ς'`).
pub(crate) fn find_plain(text: &str, query: &str) -> Vec<Range<usize>> {
    if text.is_empty() || query.is_empty() {
        return Vec::new();
    }
    if text.is_ascii() && query.is_ascii() {
        let hay = text.to_ascii_lowercase();
        let needle = query.to_ascii_lowercase();
        return hay.match_indices(&needle).map(|(at, _)| at..at + needle.len()).collect();
    }
    let fold = |s: &str| {
        let mut folded = String::with_capacity(s.len());
        let mut map = Vec::with_capacity(s.len() + 1);
        for (b, c) in s.char_indices() {
            map.push(b);
            if c == 'ς' {
                folded.push('σ');
            } else {
                for fc in c.to_lowercase() {
                    folded.push(fc);
                }
            }
            map.resize(folded.len(), usize::MAX);
        }
        map.push(s.len());
        (folded, map)
    };
    let (hay, map) = fold(text);
    let (needle, _) = fold(query);
    if needle.is_empty() {
        return Vec::new();
    }
    hay.match_indices(&needle)
        .filter_map(|(f0, _)| {
            let (s0, s1) = (*map.get(f0)?, *map.get(f0 + needle.len())?);
            (s0 != usize::MAX && s1 != usize::MAX && s0 < s1).then_some(s0..s1)
        })
        .collect()
}

impl App {
    pub fn find_focused(&self) -> bool {
        self.find.open && self.find.input.focused
    }

    /// Opens Find in the active session (or focuses and selects its query when already open).
    pub fn open_find(&mut self, tx: &mut Tx) {
        let Route::Session(id) = self.route.clone() else { return };
        let had_composer = self.composer.focused;
        self.sync_find_session(&id);
        self.close_picker(false);
        if !self.find.open {
            self.find.restore_composer = had_composer;
            self.find.open = true;
        }
        self.composer.focused = false;
        self.find.input.focused = true;
        let len = self.find.input.text.len();
        self.find.input.set_selection(0, len);
        self.sync_find_index();
        tx.instant();
    }

    /// Closes Find, clearing the query and highlights; `restore` returns focus to the composer.
    pub fn close_find(&mut self, restore: bool, tx: &mut Tx) {
        if !self.find.open {
            return;
        }
        // The bar goes as it came, at once, and the header's crumbs take its room back with it.
        tx.instant();
        let restore_composer = restore && self.find.restore_composer;
        self.find.open = false;
        self.find.input.clear();
        self.find.input.focused = false;
        self.find.input.cancel_composition();
        self.find.active = 0;
        self.find.restore_composer = false;
        self.find.matches.clear();
        self.find.rows.clear();
        self.find.indexed_query.clear();
        if !self.find.session.is_empty() {
            self.find.saved.remove(&self.find.session);
        }
        self.composer.focused = restore_composer;
    }

    /// Steps forward (`1`) or backward (`-1`) through the session's matches, wrapping around.
    pub fn step_find(&mut self, delta: i32, tx: &mut Tx) {
        self.sync_find_index();
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        let cur = self.find.active.min(n - 1) as isize;
        let next = (cur + delta as isize).rem_euclid(n as isize) as usize;
        self.set_active_hit(next);
        self.seek_active(true, tx);
    }

    fn set_active_hit(&mut self, next: usize) {
        let old = self.find.active;
        self.find.active = next;
        let old_hit = self.find.matches.get(old).cloned();
        let new_hit = self.find.matches.get(next).cloned();
        if let Some(h) = &old_hit
            && Some(&h.row_key) != new_hit.as_ref().map(|n| &n.row_key)
            && let Some(r) = self.find.rows.get_mut(&h.row_key)
        {
            r.active = None;
            r.version = row_hash(&r.ranges, None);
        }
        if let Some(h) = &new_hit
            && let Some(r) = self.find.rows.get_mut(&h.row_key)
        {
            r.active = Some(h.local);
            r.version = row_hash(&r.ranges, r.active);
        }
    }

    /// Opens a folded long user prompt when `hit` falls in or past its 40 pt fading region
    /// (`search-controller.ts`: `rect.bottom > fold.bottom - FOLD_FADE`).
    fn unfold_if_hidden(&mut self, sid: &str, hit: &Hit) -> bool {
        if !self.settings.fold_user {
            return false;
        }
        let Some(id) = hit.row_key.strip_prefix("u-") else { return false };
        let fold_key = crate::fold::key(id);
        if self.expanded.contains(&fold_key) {
            return false;
        }
        let Some(body) = self.transcripts.get(sid).and_then(|t| {
            t.messages.iter().find_map(|m| match m {
                Message::User { id: mid, text } if mid == id => Some(text.clone()),
                _ => None,
            })
        }) else {
            return false;
        };
        let shown = self.bubble_text(id, &body);
        let quotes = !opencode_kit::model::user_quotes(id).is_empty() && !body.trim().is_empty();
        let wrap_w = crate::fold::text_width(crate::fold::viewport_width(), quotes);
        if crate::fold::lines(&shown, wrap_w) <= crate::fold::OVER {
            return false;
        }
        let end = shown.ceil_char_boundary(hit.end_byte.min(shown.len()));
        let prefix = &shown[..end];
        let (_, measured_h) = crate::fold::measure(prefix, wrap_w);
        let hard_h = prefix.split('\n').count() as f32 * crate::view::BUBBLE.line_px();
        let clear = crate::fold::SHOW as f32 * crate::view::BUBBLE.line_px() - 40.0;
        if measured_h.max(hard_h) <= clear {
            return false;
        }
        self.expanded.insert(fold_key);
        true
    }

    fn seek_active(&mut self, glide: bool, tx: &mut Tx) {
        let Route::Session(sid) = self.route.clone() else { return };
        let idx = self.find.active_index();
        let Some(hit) = self.find.matches.get(idx).cloned() else { return };
        if self.unfold_if_hidden(&sid, &hit) {
            tx.instant();
        }
        let scroll_key = format!("scroll-{sid}");
        if glide {
            tx.scroll_center_in(&scroll_key, &hit.row_key);
            tx.scroll_center_in(&scroll_key, ACTIVE_KEY);
        } else {
            tx.scroll_center_in_instant(&scroll_key, &hit.row_key);
            tx.scroll_center_in_instant(&scroll_key, ACTIVE_KEY);
        }
    }

    fn sync_find_session(&mut self, current: &str) {
        if self.find.session == current {
            return;
        }
        if !self.find.session.is_empty() {
            if self.find.open || !self.find.input.text.is_empty() {
                self.find.saved.insert(
                    self.find.session.clone(),
                    Saved {
                        open: self.find.open,
                        query: self.find.input.text.clone(),
                        active: self.find.active,
                        restore_composer: self.find.restore_composer,
                    },
                );
            } else {
                self.find.saved.remove(&self.find.session);
            }
        }
        self.find.session = current.to_owned();
        let restored = self.find.saved.get(current).cloned().unwrap_or_default();
        self.find.open = restored.open;
        self.find.input = TextEdit::new(restored.query);
        self.find.active = restored.active;
        self.find.restore_composer = restored.restore_composer;
        self.find.matches.clear();
        self.find.rows.clear();
        self.find.indexed_session.clear();
    }

    pub(crate) fn sync_find_index(&mut self) {
        let current = match &self.route {
            Route::Session(id) if !self.settings.open => id.clone(),
            _ => String::new(),
        };
        self.sync_find_session(&current);
        if !self.find.open || current.is_empty() {
            self.find.matches.clear();
            self.find.rows.clear();
            self.find.indexed_query.clear();
            return;
        }
        let q = self.find.query().to_lowercase();
        if q.is_empty() {
            self.find.matches.clear();
            self.find.rows.clear();
            self.find.indexed_query.clear();
            self.find.parts.clear();
            return;
        }
        let (shown, has_revert, fp) = {
            let revert = self.session(&current).and_then(|i| i.revert.as_deref());
            let Some(t) = self.transcripts.get(&current) else {
                self.find.matches.clear();
                self.find.rows.clear();
                return;
            };
            let shown = t.visible_len(revert);
            (shown, revert.is_some(), transcript_fp(t, shown))
        };
        if self.find.indexed_session == current && self.find.indexed_query == q && self.find.indexed_fp == fp {
            return;
        }
        if self.find.indexed_session != current || self.find.indexed_query != q {
            self.find.parts.clear();
        }
        self.find.indexed_session = current.clone();
        self.find.indexed_query = q.clone();
        self.find.indexed_fp = fp;

        // Scan searchable rows in exact timeline order (`u-{id}` and `t-{a.id}-{n}`) without
        // cloning message strings.
        let mut matches = Vec::new();
        let mut rows = HashMap::new();
        let mut record = |parts: &mut HashMap<(String, usize), CachedPart>, row_key: String, cache_key: (String, usize), body: &str, plain: bool| {
            let sig = str_sig(body);
            let len = body.len();
            let ranges = if let Some(cached) = parts.get(&cache_key).filter(|c| c.len == len && c.head_tail == sig) {
                cached.ranges.clone()
            } else {
                let computed: Vec<Vec<Range<usize>>> = if plain {
                    find_plain(body, &q).into_iter().map(|r| vec![r]).collect()
                } else {
                    crate::genui::find(body, &q)
                };
                parts.insert(cache_key, CachedPart { len, head_tail: sig, ranges: computed.clone() });
                computed
            };
            if !ranges.is_empty() {
                for (local, rs) in ranges.iter().enumerate() {
                    let end_byte = rs.last().map_or(0, |r| r.end);
                    matches.push(Hit { row_key: row_key.clone(), local, end_byte });
                }
                let version = row_hash(&ranges, None);
                rows.insert(row_key, RowHits { ranges, active: None, version });
            }
        };
        let mut parts = std::mem::take(&mut self.find.parts);
        if let Some(t) = self.transcripts.get(&current) {
            let busy = t.busy() && !self.stops.has(&current);
            let mut steps: Vec<&Assistant> = Vec::new();
            let flush_steps = |steps: &[&Assistant], live: bool, parts: &mut HashMap<(String, usize), CachedPart>, record: &mut dyn FnMut(&mut HashMap<(String, usize), CachedPart>, String, (String, usize), &str, bool)| {
                for (mi, a) in steps.iter().enumerate() {
                    let live_msg = live && mi + 1 == steps.len();
                    let mut texts = 0usize;
                    for (pi, part) in a.parts.iter().enumerate() {
                        if let Part::Text { text } = part {
                            let n = texts;
                            texts += 1;
                            let is_last = live_msg && pi + 1 == a.parts.len();
                            if text.trim().is_empty() && !is_last {
                                continue;
                            }
                            record(parts, format!("t-{}-{n}", a.id), (a.id.clone(), n), text, false);
                        }
                    }
                }
            };
            for m in &t.messages[..shown] {
                match m {
                    Message::Assistant(a) => steps.push(a),
                    Message::User { id, text } => {
                        flush_steps(&std::mem::take(&mut steps), false, &mut parts, &mut record);
                        if !text.trim().is_empty() || !opencode_kit::model::user_quotes(id).is_empty() {
                            let shown_text = self.bubble_text(id, text);
                            record(&mut parts, format!("u-{id}"), (id.clone(), 0), &shown_text, true);
                        }
                    }
                    Message::Note { .. } | Message::Compaction(_) | Message::Notice(_) => {
                        flush_steps(&std::mem::take(&mut steps), false, &mut parts, &mut record);
                    }
                }
            }
            flush_steps(&steps, busy && !has_revert, &mut parts, &mut record);
        }
        self.find.parts = parts;
        self.find.matches = matches;
        self.find.rows = rows;
        if !self.find.matches.is_empty() {
            let active = self.find.active.min(self.find.matches.len() - 1);
            self.set_active_hit(active);
        } else {
            self.find.active = 0;
        }
    }
}

/// Called from `find-input`'s `on_input_change`: updates matches and seeks the first hit in the
/// keystroke's own commit.
pub fn typed(s: &mut App, tx: &mut Tx) {
    s.find.active = 0;
    s.sync_find_index();
    if s.find.matches.is_empty() {
        tx.instant();
        return;
    }
    s.set_active_hit(0);
    s.seek_active(false, tx);
}

pub fn poll(s: &mut App) -> bool {
    let cur = match &s.route {
        Route::Session(id) if !s.settings.open => id.as_str(),
        _ => "",
    };
    if !s.find.open && s.find.session == cur && !s.find.saved.contains_key(cur) {
        return false;
    }
    let was_fp = (s.find.indexed_fp, s.find.matches.len());
    s.sync_find_index();
    s.find.open && (s.find.indexed_fp, s.find.matches.len()) != was_fp
}

/// Global Find shortcuts: ⌘F opens Find in a session or focuses session search on Home;
/// ⌘G and ⇧⌘G step while session Find is open.
pub fn shortcut(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.settings.open || s.opening.is_some() {
        return false;
    }
    if Bind::Find.matches_tx(ev, tx) {
        return match s.route {
            Route::Session(_) => { s.open_find(tx); true }
            Route::Home => { s.focus_home_search(tx); true }
            Route::Draft(_) => false,
        };
    }
    let step = [(Bind::FindNext, 1), (Bind::FindPrev, -1)].into_iter().find(|(b, _)| b.matches_tx(ev, tx));
    if let Some((_, d)) = step.filter(|_| matches!(s.route, Route::Session(_)) && s.find.open) {
        s.step_find(d, tx);
        return true;
    }
    false
}

/// Escape when Find is open (outside a busy composer's stop).
pub fn dismiss_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let take = ev.key == Key::Escape && s.find.open && matches!(s.route, Route::Session(_));
    // Desktop leaves nothing focused once Escape closes the bar (`esc-find`).
    if take { s.close_find(false, tx); }
    take
}

fn input_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.find.input.is_composing() {
        return false;
    }
    if ev.key == Key::Escape {
        s.close_find(false, tx);
        return true;
    }
    let m = ev.mods;
    if !m.alt && !m.cmd && !m.ctrl {
        let step = match ev.key {
            Key::Enter => Some(if m.shift { -1 } else { 1 }),
            Key::Down if !m.shift => Some(1),
            Key::Up if !m.shift => Some(-1),
            _ => None,
        };
        if let Some(d) = step {
            s.step_find(d, tx);
            return true;
        }
    }
    s.switcher_key(ev, tx) || s.shortcut(ev, tx)
}

/// The session header's Find bar (`TimelineSearchBar`).
pub fn bar(s: &App, p: &Pal) -> Option<El> {
    if !s.find.open {
        return None;
    }
    let has_query = !s.find.query().is_empty();
    let total = s.find.count();
    let count = has_query.then(|| {
        let label = if total > 0 { format!("{} of {total}", s.find.active_index() + 1) } else { "0 of 0".into() };
        text(label).key("find-count").font(COUNT).color(p.muted)
    });
    let border = if s.find.input.focused { p.q.border_strong } else { p.border };
    Some(
        row()
            .key("find-bar")
            .transition(Transition::none())
            .h(28.0)
            .w(200.0)
            .pl(8.0)
            .pr(4.0)
            .gap(6.0)
            .center_y()
            .radius(6.0)
            .fill(p.bg_base)
            .box_shadow(Pal::ring_out(border))
            .child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted))
            .child(
                text_input(&s.find.input, UI)
                    .key("find-input")
                    .spellcheck(s.settings.spellcheck)
                    .single_line()
                    .truncate_fade()
                    .grow(1.0)
                    .shrink()
                    .min_w(0.0)
                    .h(20.0)
                    .color(p.text)
                    .placeholder(opencode_i18n::t!("session.search.placeholder"), p.faint)
                    .caret(p.text, p.selection).selection_style(p.field_selection)
                    .on_edit(|s: &mut App| &mut s.find.input)
                    .on_input_change(typed)
                    .on_input_key(input_key),
            )
            .children(count)
            .child(
                row()
                    .key("find-close")
                    .role("button")
                    .label(opencode_i18n::t!("rune.find.close"))
                    .help(opencode_i18n::t!("rune.find.close"))
                    .size(20.0, 20.0)
                    .center()
                    .radius(4.0)
                    .hover(p.hover)
                    .press(p.pressed)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.close_find(true, tx))
                    .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted)),
            ),
    )
}

/// Native macOS Edit menu items (`keys_first` so focused fields and `on_key` see them first),
/// with the keys their commands have now.
pub fn menu_items() -> Vec<MenuItem<App>> {
    let item = |title, id, key: Bind, f: fn(&mut App, &mut Tx)| {
        crate::menubar::keyed(MenuItem::new(title, f).id(id).menu("Edit"), key).keys_first()
    };
    [
        item(opencode_i18n::t!("rune.menu.edit.find"), "Edit/Find", Bind::Find, |s, tx| match s.route {
            Route::Session(_) => s.open_find(tx),
            Route::Home => s.focus_home_search(tx),
            Route::Draft(_) => {}
        }),
        item(opencode_i18n::t!("rune.menu.edit.findNext"), "Edit/FindNext", Bind::FindNext, |s, tx| s.step_find(1, tx)),
        item(opencode_i18n::t!("rune.menu.edit.findPrevious"), "Edit/FindPrevious", Bind::FindPrev, |s, tx| s.step_find(-1, tx)),
    ]
    .into()
}
