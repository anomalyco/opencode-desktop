//! `⌘P` Open File modal palette (`use-session-commands.tsx` `file.open` + `DialogCommandPalette`).

use crate::pickers::PickerHost as _;
use std::borrow::Cow;
use std::time::Duration;

use rune::{
    Anim, Color, Cursor, El, GrowthSpec, Key, KeyEvent, Latest, ScrollSpec, TextEdit, Tx,
    Waker, col, el, icon, row, text, text_input,
};
use rune_kit::diff::FileStatus;
use rune_kit::file_icon;
use rune_theme_opencode::icons as ic;

use super::data::{fetch_live_find, parse_path_and_line, rank_file_query};
use crate::palette::Pal;
use crate::view::{UI, UI_MED, UI_SMALL};
use crate::{App, Route, Source};

const INSTANT: Anim = Anim::INSTANT;
const ROW_H: f32 = 34.0;
const MAX_EMPTY: usize = 30;

pub struct FilePalette {
    pub open: bool,
    pub search: TextEdit,
    pub active_idx: usize,
    last_query: String,
    restore_composer: bool,
    pub found: Vec<String>,
    asked: Option<(String, String)>,
    fetch: Latest<(String, Vec<String>)>,
}

impl Default for FilePalette {
    fn default() -> Self {
        Self {
            open: false,
            search: TextEdit::new(""),
            active_idx: 0,
            last_query: String::new(),
            restore_composer: false,
            found: Vec::new(),
            asked: None,
            fetch: Latest::default().debounce(Duration::from_millis(90)),
        }
    }
}

impl FilePalette {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch.set_waker(waker);
    }
}

impl App {
    pub fn open_file_palette(&mut self, tx: &mut Tx) {
        if !matches!(self.route, Route::Session(_)) {
            return;
        }
        tx.instant();
        if self.files.palette.open {
            self.files.palette.search.focused = true;
            return;
        }
        let restore = self.composer.focused
            || (self.switcher.open && self.switcher.restore_composer())
            || (self.project_switcher.open && self.project_switcher.restore_composer());
        self.close_picker(false);
        self.close_switcher(false);
        self.close_project_switcher(false);
        self.menus.edit.focused = false;
        self.find.input.focused = false;
        self.composer.focused = false;

        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            self.ensure_workspace_files(&sid);
            self.ensure_session_review(&sid);
        }

        self.files.palette.restore_composer = restore;
        self.files.palette.open = true;
        self.files.palette.search = TextEdit::new("");
        self.files.palette.search.focused = true;
        self.files.palette.active_idx = 0;
        self.files.palette.last_query.clear();
        self.files.palette.found.clear();
        self.files.palette.asked = None;
        self.files.palette.fetch.cancel();
    }

    pub fn close_file_palette(&mut self, restore: bool) {
        if !self.files.palette.open {
            return;
        }
        self.files.palette.open = false;
        self.files.palette.search.focused = false;
        self.files.palette.search.preedit = None;
        self.files.palette.fetch.cancel();
        if restore && self.files.palette.restore_composer {
            self.composer.focused = true;
        }
    }

    pub fn poll_file_palette(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some((q, list)) = self.files.palette.fetch.take()
            && self.files.palette.asked.as_ref().map(|(_, aq)| aq.as_str()) == Some(q.as_str())
        {
            tx.instant();
            self.files.palette.found = list;
            changed = true;
        }
        if !self.files.palette.open {
            return changed;
        }
        let raw_q = self.files.palette.search.text.clone();
        if raw_q != self.files.palette.last_query {
            self.files.palette.last_query = raw_q.clone();
            self.files.palette.active_idx = 0;
            changed = true;
        }
        let (clean_q, _) = parse_path_and_line(&raw_q);
        let clean = clean_q.trim().to_owned();
        if !clean.is_empty()
            && let Source::Live { server, .. } = &self.source
            && let Some(sid) = self.active_session_id()
        {
            let dir = self.session(sid).map(|s| s.directory.clone()).unwrap_or_default();
            let ask = (dir.clone(), clean.clone());
            if self.files.palette.asked.as_ref() != Some(&ask) {
                self.files.palette.asked = Some(ask);
                let srv = server.clone();
                self.files
                    .palette
                    .fetch
                    .request((dir, clean), move |(dir, q)| {
                        let found = fetch_live_find(&srv, &dir, &q);
                        (q, found)
                    });
            }
        }
        changed
    }

    pub fn file_palette_entries(&self) -> (Vec<String>, Option<u32>) {
        let Some(sid) = self.active_session_id() else {
            return (Vec::new(), None);
        };
        let raw_q = &self.files.palette.search.text;
        let (clean_q, jump_line) = parse_path_and_line(raw_q);
        let q = clean_q.trim();

        let ws = self.files.sessions.get(sid);
        if q.is_empty() {
            let mut out = Vec::new();
            let mut push_unique = |p: &str| {
                if out.len() < MAX_EMPTY && !p.is_empty() && !out.iter().any(|x| x == p) {
                    out.push(p.to_owned());
                }
            };
            if let Some(ws) = ws {
                for r in ws.recent.iter().rev() {
                    push_unique(r);
                }
            }
            if let Some(sr) = self.review.sessions.get(sid) {
                for m in &sr.meta {
                    push_unique(&m.path);
                }
            }
            if let Some(ws) = ws {
                for f in &ws.known_files {
                    push_unique(f);
                }
            }
            out.truncate(MAX_EMPTY);
            return (out, jump_line);
        }

        let mut pool: Vec<String> = Vec::new();
        for p in &self.files.palette.found {
            if !pool.contains(p) {
                pool.push(p.clone());
            }
        }
        if let Some(ws) = ws {
            for p in &ws.known_files {
                if !pool.contains(p) {
                    pool.push(p.clone());
                }
            }
        }
        if let Some(sr) = self.review.sessions.get(sid) {
            for m in &sr.meta {
                if !pool.contains(&m.path) {
                    pool.push(m.path.clone());
                }
            }
        }
        let mut ranked = rank_file_query(&pool, q);
        ranked.truncate(80);
        (ranked, jump_line)
    }

    pub fn file_palette_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if crate::keymap::Bind::FileOpen.matches_tx(ev, tx)
            && matches!(self.route, Route::Session(_))
            && !self.settings.open
        {
            if self.files.palette.open {
                tx.instant();
                self.close_file_palette(true);
            } else {
                self.open_file_palette(tx);
            }
            return true;
        }
        if !self.files.palette.open {
            return false;
        }
        if self.files.palette.search.preedit.is_some() {
            return false;
        }
        let (entries, jump_line) = self.file_palette_entries();
        let len = entries.len();
        match &ev.key {
            Key::Escape => {
                tx.instant();
                self.close_file_palette(true);
                true
            }
            Key::Down if len > 0 => {
                tx.instant();
                self.files.palette.active_idx = (self.files.palette.active_idx + 1) % len;
                tx.scroll_into_view_in(
                    "file-palette-scroll",
                    format!("fp:{}", entries[self.files.palette.active_idx]),
                );
                true
            }
            Key::Up if len > 0 => {
                tx.instant();
                self.files.palette.active_idx =
                    (self.files.palette.active_idx + len - 1) % len;
                tx.scroll_into_view_in(
                    "file-palette-scroll",
                    format!("fp:{}", entries[self.files.palette.active_idx]),
                );
                true
            }
            Key::Home if len > 0 => {
                tx.instant();
                self.files.palette.active_idx = 0;
                tx.scroll_to_y("file-palette-scroll", 0.0);
                true
            }
            Key::End if len > 0 => {
                tx.instant();
                self.files.palette.active_idx = len - 1;
                true
            }
            Key::Enter if !ev.mods.shift => {
                if let Some(path) = entries
                    .get(self.files.palette.active_idx)
                    .or_else(|| entries.first())
                    .cloned()
                {
                    self.close_file_palette(false);
                    self.open_workspace_file(&path, false, jump_line, tx);
                }
                true
            }
            _ => false,
        }
    }
}

pub fn view(s: &App, p: &Pal) -> Option<El> {
    if !s.files.palette.open {
        return None;
    }
    let sid = s.active_session_id().unwrap_or("");
    let (entries, jump_line) = s.file_palette_entries();
    let active_idx = s.files.palette.active_idx.min(entries.len().saturating_sub(1));

    let search_bar = row()
        .key("file-palette-header")
        .w_full()
        .h(46.0)
        .px(14.0)
        .gap(10.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 15.0).color(p.icon_muted))
        .child(
            text_input(&s.files.palette.search, UI)
                .key("file-palette-input")
                .placeholder("Open file by name (or path:line)…", p.faint)
                .color(p.text)
                .grow(1.0)
                .shrink()
                .on_edit(|s: &mut App| &mut s.files.palette.search)
                .on_submit(move |s: &mut App, tx: &mut Tx| {
                    let (list, line) = s.file_palette_entries();
                    if let Some(path) = list
                        .get(s.files.palette.active_idx)
                        .or_else(|| list.first())
                        .cloned()
                    {
                        s.close_file_palette(false);
                        s.open_workspace_file(&path, false, line, tx);
                    }
                }),
        )
        .children(jump_line.map(|ln| {
            el().px(6.0)
                .py(2.0)
                .radius(4.0)
                .fill(p.layer2)
                .child(text(format!(":{ln}")).font(UI_SMALL).color(p.muted))
        }));

    let body = if entries.is_empty() {
        col()
            .key("file-palette-empty")
            .w_full()
            .h(120.0)
            .center()
            .child(text("No matching files").font(UI_SMALL).color(p.muted))
    } else {
        let count = entries.len();
        let keys_vec = entries.clone();
        let rows_vec = entries;
        let pal = *p;
        let sid_owned = sid.to_owned();
        let list = col()
            .key("file-palette-rows")
            .w_full()
            .padding(6.0)
            .growth(GrowthSpec::default())
            .role("listbox")
            .virtual_rows(
                count,
                ROW_H,
                move |_s: &App, i| Cow::Owned(format!("fp:{}", keys_vec[i])),
                move |s: &App, i| {
                    let path = &rows_vec[i];
                    let active = i == active_idx;
                    let (dir, name) = match path.rsplit_once('/') {
                        Some((d, b)) => (Some(d), b),
                        None => (None, path.as_str()),
                    };
                    let status = s
                        .review
                        .sessions
                        .get(&sid_owned)
                        .and_then(|sr| sr.meta.iter().find(|m| &m.path == path))
                        .map(|m| m.status);
                    let click_path = path.clone();
                    let mut r = row()
                        .w_full()
                        .h(ROW_H)
                        .px(10.0)
                        .gap(8.0)
                        .center_y()
                        .radius(6.0)
                        .fill(if active { pal.row_active } else { Color::TRANSPARENT })
                        .hover(pal.hover)
                        .hover_motion(INSTANT, INSTANT)
                        .cursor(Cursor::Pointer)
                        .role("option")
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                            let (_, line) = s.file_palette_entries();
                            s.close_file_palette(false);
                            s.open_workspace_file(&click_path, false, line, tx);
                        })
                        .child(icon(file_icon(name), 15.0).color(if active { pal.icon } else { pal.icon_muted }))
                        .child(
                            text(name.to_owned())
                                .font(if active { UI_MED } else { UI })
                                .color(pal.text)
                                .shrink()
                                .min_w(0.0)
                                .truncate_fade(),
                        );
                    if let Some(d) = dir {
                        r = r.child(
                            text(d.to_owned())
                                .font(UI_SMALL)
                                .color(pal.faint)
                                .grow(1.0)
                                .shrink()
                                .min_w(0.0)
                                .truncate_fade(),
                        );
                    } else {
                        r = r.child(el().grow(1.0));
                    }
                    if let Some(st) = status {
                        let (lbl, col_fg) = match st {
                            FileStatus::Added => ("A", pal.success),
                            FileStatus::Deleted => ("D", pal.danger),
                            _ => ("M", pal.warning),
                        };
                        r = r.child(text(lbl).font(UI_SMALL).color(col_fg));
                    }
                    r
                },
            );
        col()
            .key("file-palette-scroll")
            .w_full()
            .max_h(340.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(list)
    };

    let modal = col()
        .key("file-palette-modal")
        .w(520.0)
        .mt(76.0)
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(8.0, 28.0, p.shadow_far)
        .shadow(2.0, 6.0, p.shadow)
        .on_press(|_: &mut App| {})
        .child(search_bar)
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(body);

    Some(
        col()
            .key("file-palette-scrim")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .items_center()
            .fill(p.scrim)
            .motion(rune::Spring::NONE)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.close_file_palette(true);
            })
            .child(modal),
    )
}
