//! The palette drawn as Desktop's `CommandPaletteView` (`shell/commands/dialog.tsx`): the compact
//! dialog (`dialog.css:141-234`) over `SearchList` (`ui/forms/search-list/search-list.css`), its
//! rows `PaletteRow` with `palette-menu.css`'s title and `keybind.css`'s chips.

use std::hash::{DefaultHasher, Hash, Hasher};

use opencode_i18n::t;
use opencode_kit::model::{Project, SessionInfo};
use opencode_kit::servers::ServerKey;
use opencode_search::fulltext::{Kind as FullTextKind, Snippet};
use opencode_ui::focus::FocusExt as _;
use opencode_ui::keymap::{LABEL as KEYMAP_LABEL, caps as keymap_caps, chips as keymap_chips};
use opencode_ui::widgets::DialogSize;
use rune::{Color, Cx, Direction, El, PointerEvent, Span, Spring, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::{file_icons, icons as ic};
use opencode_ui::palette::{Elevate as _, Pal};
use opencode_ui::style::{UI, UI_SMALL, activity_ring, field, touch_controls};

use crate::entries::{Action, Entry, active, empty, entries};
use crate::shown::Empty;
use crate::{FullTextRow, SwitcherHost, SwitcherMode, ui_direction};

/// The search block (`search-list-search` in the compact palette, `dialog.css:156-166`): 6 above
/// the 28-tall field, 17 below it, with the hairline 8 from the block's foot.
const HEAD_H: f32 = 6.0 + 28.0 + 9.0;
/// A compact row (`search-list-row[data-size=compact]`, `dialog.css:202`).
const ROW_H: f32 = 32.0;
/// Compact section header (`dialog.css:190-198`: `padding-block: 10px 2px`, `line-height: 18px`).
const SECTION_HEADER_H: f32 = 30.0;
/// Non-last section bottom padding (`dialog.css:185-187`: `padding-bottom: 4px`).
const SECTION_PB: f32 = 4.0;
/// A row whose description goes under its title in a window 640 pt or narrower (`min-height: 40px`).
const STACKED_H: f32 = 40.0;

fn entry_height(stacked: bool) -> f32 {
    if touch_controls() {
        44.0
    } else if stacked {
        STACKED_H
    } else {
        ROW_H
    }
}

/// A session only its messages matched: its title, and under it the line of context (6 above
/// and below, the title's 18 and the line's 16).
const SNIPPET_H: f32 = 46.0;

/// `e`'s row height: with its line of context, stacked, or one line.
fn height(s: &dyn SwitcherHost, e: &Entry, q: &str, fulltext: &[FullTextRow]) -> f32 {
    if snippet(e, q, fulltext).is_some() { SNIPPET_H.max(entry_height(false)) } else { entry_height(stacked(s, e)) }
}

/// The full text row a session's palette row shows under its title: when its messages matched
/// and its title doesn't hold the query.
fn snippet<'a>(e: &Entry, q: &str, fulltext: &'a [FullTextRow]) -> Option<&'a FullTextRow> {
    let Action::Session(id) = &e.action else { return None };
    let found = fulltext.iter().find(|r| r.session == *id)?;
    let fold = opencode_search::SearchFold::for_query(q.trim());
    (!fold.fold(&e.title).contains(&fold.fold(q.trim()))).then_some(found)
}

/// Whether `e`'s row stacks its description under its title: the window is 640 pt or narrower
/// and it has one (a file's row keeps its own line).
fn stacked(s: &dyn SwitcherHost, e: &Entry) -> bool {
    let w = s.narrow_width();
    w > 0.0 && w <= 640.0 && !e.description.is_empty() && !matches!(e.action, Action::File(_))
}

/// Rows and the header sit 4 apart in a section, and a section ends 4 above the next
/// (`dialog.css:196-202`).
const GAP: f32 = 2.0;
/// `search-list-empty`'s height.
const EMPTY_H: f32 = 120.0;
/// `command-palette-meta` (`packages/app/src/shell/commands/dialog.css:62-76`): 11/16 at 440,
/// +0.05 tracking, tabular figures.
const META: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0).tracking(0.05).features("\"tnum\"");

/// The first match of the query in `s`, in the accent colour (`SearchHighlight`).
pub fn highlighted(s: &str, q: &str, style: TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate_fade();
    match match_span(s, q, accent) {
        Some(span) => t.spans(vec![span]),
        None => t,
    }
}

/// [`highlighted`]'s span: the first match of the query in `s`.
fn match_span(s: &str, q: &str, accent: Color) -> Option<Span> {
    let lower = s.to_lowercase();
    let needle = q.trim().to_lowercase();
    match lower.find(&needle) {
        Some(at) if !needle.is_empty() && lower.len() == s.len() => Some(Span::new(at, at + needle.len()).color(accent)),
        _ => None,
    }
}

/// `s`'s width as a text that inherits `dir` draws it, so the measure and the drawn text share
/// one shaped layout.
fn width_in(s: &str, spans: &[Span], style: TextStyle, dir: Direction) -> f32 {
    rune::text::measure_with(s, spans, style, None, rune::text::RichLayoutOptions::new().direction(dir)).0
}

/// The scrim, and the dialog where Desktop puts it: `<Dialog size="large" fit>`
/// (`shell/commands/dialog.tsx`), as tall as its rows and growing down from the top edge.
pub fn view<H: SwitcherHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> [El; 2] {
    let [win_w, win_h] = cx.layout_size("window").or_else(|| cx.layout_size("zoomed")).unwrap_or([1280.0, 820.0]);
    let size = DialogSize::Large;
    let w = size.width(win_w);
    let scrim = el().key("switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog(s, p, w, size.height(win_h)).absolute((win_w - w) / 2.0, size.top(win_h))]
}

/// The dialog itself, `w` wide and as tall as its rows up to `max_h`.
pub fn dialog<H: SwitcherHost + 'static>(s: &H, p: &Pal, w: f32, max_h: f32) -> El {
    let host: &dyn SwitcherHost = s;
    let q = host.switcher().query();
    let rows = entries(host);
    let lit_key = active(host, &rows);
    let fulltext = host.palette_fulltext_rows();
    // The page pool coalesces this request. Use the row already ranked for this frame;
    // polling just to rediscover it used to run the entire search a second time.
    if let Some(id) = lit_key.as_deref().and_then(|key| key.strip_prefix("session:")) {
        host.prefetch_session(id);
    }
    let placeholder = dialog_placeholder(host);
    let input = search_input_shell(host, p, placeholder, lit_key.as_deref())
        .on_edit(|s: &mut H| &mut s.switcher_mut().search)
        .on_input_change(|s: &mut H, _: &mut Tx| s.switcher_mut().query_changed())
        .on_input_key(|s: &mut H, ev, tx| crate::nav_or_shortcut(s, ev, tx));
    let search = search_row_shell(p, input);

    let mut results = col().key("switcher-results").w_full().px(6.0).pb(6.0);
    let mut content_h = 0.0_f32;
    // Only rows near the scroll view are built (`band`); the highlighted one always is.
    let window = host.switcher().band.window(max_h - HEAD_H - entry_height(false) + ROW_H);
    let mode = host.switcher().mode;
    let mut i = 0;
    while i < rows.len() {
        let group = rows[i].group;
        let end = rows[i..].iter().position(|e| e.group != group).map_or(rows.len(), |n| i + n);
        let last = end == rows.len();
        let bare = i == 0 && last && matches!(mode, SwitcherMode::Commands | SwitcherMode::Sessions | SwitcherMode::Archived);
        let mut section = section_shell(p, group, bare, last);
        let size = end - i;
        let mut top = content_h + if bare { 4.0 } else { SECTION_HEADER_H + GAP };
        // Rows left out since the last built one: their count and heights.
        let mut skipped = (0usize, 0.0_f32);
        for (n, e) in rows[i..end].iter().enumerate() {
            let h = height(host, e, q, &fulltext);
            let lit = lit_key.as_deref() == Some(e.key.as_str());
            if lit || (top < window.end && top + h > window.start) {
                if skipped.0 > 0 {
                    section = section.child(gap_for(group, n, skipped));
                    skipped = (0, 0.0);
                }
                section = section.child(memo_row(s, p, e, lit, q, w, (n + 1, size), &fulltext));
            } else {
                skipped = (skipped.0 + 1, skipped.1 + h);
            }
            top += h + GAP;
        }
        if skipped.0 > 0 {
            section = section.child(gap_for(group, size, skipped));
        }
        content_h = top - GAP + if last { 0.0 } else { SECTION_PB };
        results = results.child(section);
        i = end;
    }
    host.switcher().band.built(&window);
    let (results, content_h) = finish_results(host, p, results, rows.is_empty(), content_h);
    let header = HEAD_H + entry_height(false) - ROW_H;
    let scroll_h = content_h.min(max_h - header).max(0.0);
    // `MenuScroll`: the thumb shows while the list scrolls (`search-list.tsx:311`).
    let scroll = opencode_ui::scroller::scroller("switcher-scroll")
        .bar(opencode_ui::scroller::Bar::MenuScroll)
        .build::<H>(p, scroll_box(host, scroll_h), results);
    build_dialog_card(p, placeholder, w, header + scroll_h, search, scroll).modal(true, |s: &mut H| crate::close(s, true))
}

fn dialog_placeholder(s: &dyn SwitcherHost) -> &'static str {
    match s.switcher().mode {
        SwitcherMode::Sessions | SwitcherMode::Archived => t!("home.sessions.search.placeholder"),
        SwitcherMode::QuickOpen => t!("palette.quickOpen.placeholder"),
        SwitcherMode::Commands => t!("command.commands.search"),
        // `palette.search.placeholder.home` on Home (`HomeCommandPalette`).
        _ if s.is_home() => t!("palette.search.placeholder.home"),
        _ => t!("palette.search.placeholder"),
    }
}

fn search_input_shell(s: &dyn SwitcherHost, p: &Pal, placeholder: &'static str, active: Option<&str>) -> El {
    // `text-input-v2` in the compact palette: 28 tall and transparent, the glyph 6 in and 6 from
    // the text, the text in `text-base` and the placeholder in it at half strength.
    // The highlighted row is the field's active descendant: arrows keep focus in the field, and
    // a reader says each row they land on (`SearchList`'s `aria-activedescendant`).
    let input = field(&s.switcher().search, UI)
        .key("switcher-input")
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder(placeholder, p.text.alpha(p.text.a * 0.5))
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .label(placeholder);
    match active {
        Some(k) => input.active_descendant(format!("sw-{k}")),
        None => input,
    }
}

fn search_row_shell(p: &Pal, input: El) -> El {
    row()
        .key("switcher-field")
        .h(entry_height(false) - 4.0)
        .pl(6.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(input)
}

fn section_shell(p: &Pal, group: &'static str, bare: bool, last: bool) -> El {
    // Space inside the section, so the next header meets this one and pushes it up. For a
    // reader the section is a group named by its header, and the header only a picture.
    col()
        .key(format!("sw-section-{group}"))
        .w_full()
        .pt(if bare { 4.0 } else { 0.0 })
        .gap(GAP)
        .pb(if last { 0.0 } else { SECTION_PB })
        .role("group")
        .label(group)
        .children((!bare).then(|| {
            row()
                .key("header")
                .w_full()
                .h(SECTION_HEADER_H)
                .pt(10.0)
                .pb(2.0)
                .px(8.0)
                .fill(p.bg_base)
                .sticky_top(0.0)
                .a11y_hidden()
                .child(text(group).font(TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05)).color(p.faint))
        }))
}

fn finish_results(s: &dyn SwitcherHost, p: &Pal, mut results: El, is_empty: bool, mut content_h: f32) -> (El, f32) {
    let partial = s.palette_indexing_history();
    if is_empty && !partial {
        match empty(s) {
            // Still searching with nothing to show: a quiet line where the first section's
            // header goes, blank until the wait is long enough to say so.
            waiting @ (Empty::Blank | Empty::Loading) => {
                let said = (waiting == Empty::Loading).then(|| text(t!("common.loading")).font(UI).color(p.muted));
                results = results.child(row().key("switcher-loading").w_full().h(ROW_H).px(6.0).center_y().children(said));
                content_h = ROW_H + 6.0;
            }
            done => {
                let said = if done == Empty::Failed { t!("common.requestFailed") } else { t!("palette.empty") };
                results = col().key("switcher-results").w_full().child(col().key("switcher-empty").w_full().h(EMPTY_H).center().child(text(said).font(UI).color(p.muted)));
                content_h = EMPTY_H;
            }
        }
    } else {
        content_h += 6.0;
    }
    if partial {
        results = results.child(
            row()
                .key("switcher-indexing")
                .role("status")
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .center_y()
                .child(text(t!("rune.search.partial")).font(UI_SMALL).color(p.muted)),
        );
        content_h += ROW_H;
    }
    (results, content_h)
}

fn scroll_box(s: &dyn SwitcherHost, scroll_h: f32) -> El {
    // `search-list.css:63-92`, `search-list.tsx:141-163`: rows fade out over 24 past the
    // bottom edge and under the stuck section header, each linear, at full strength
    // once 15 are hidden.
    col()
        .role("listbox")
        .w_full()
        .h(scroll_h)
        .scroll_fade(rune::ScrollFade::strength(24.0).linear().below_sticky())
        .on_scroll_offset(s.switcher().band.watch(s.waker()))
}

fn build_dialog_card(p: &Pal, placeholder: &'static str, w: f32, total_h: f32, search: El, scroll: El) -> El {
    col()
        .key("switcher")
        .role("dialog")
        .label(placeholder)
        .w(w)
        .h(total_h)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .child(col().key("switcher-search").w_full().pt(6.0).px(6.0).child(search))
        // `::after`: a 1 pt rule drawn at half its height (`scaleY(0.5)`) across the dialog
        // (`commands/dialog.css:152-158`).
        .child(rune::hairline(p.border).key("switcher-divider").w_full().mt(6.0).mb(2.0))
        .child(scroll)
}

/// `count` rows left unbuilt before row `n` of `group`, as tall as they and the gaps between them.
fn gap_for(group: &str, n: usize, (count, h): (usize, f32)) -> El {
    el().key(format!("sw-gap-{group}-{n}")).w_full().h(h + (count - 1) as f32 * GAP).a11y_hidden()
}

/// A landing may change a few visible rows; re-use the rest, including their handlers. Use the
/// actual age string and match spans, not a ticking clock or the generation of the whole list.
#[allow(clippy::too_many_arguments)]
fn memo_row<H: SwitcherHost + 'static>(s: &H, p: &Pal, e: &Entry, lit: bool, q: &str, w: f32, place: (usize, usize), fulltext: &[FullTextRow]) -> El {
    let hash = memo_hash(s, p, e, lit, q, w, place, fulltext);
    rune::memo(format!("palette-row-{}", e.key), hash, || entry_row(s, p, e, lit, q, w, fulltext).position_in_set(place.0, place.1))
}

#[allow(clippy::too_many_arguments)]
fn memo_hash(s: &dyn SwitcherHost, p: &Pal, e: &Entry, lit: bool, q: &str, w: f32, place: (usize, usize), fulltext: &[FullTextRow]) -> u64 {
    let mut v = DefaultHasher::new();
    // Not the query itself: what the row draws from it, its highlights and its line of context, so
    // a row a new query leaves as it was is kept.
    (e, w.to_bits(), lit, place, stacked(s, e), touch_controls()).hash(&mut v);
    let mark = |t: &str| match_span(t, q, Color::TRANSPARENT).map(|m| (m.start, m.end));
    match &e.action {
        Action::File(path) => (mark(&format!("{}/", opencode_kit::path::parent(path))), mark(opencode_kit::path::basename(path))).hash(&mut v),
        _ => (mark(&e.title), mark(&e.description)).hash(&mut v),
    }
    let found_snippet = snippet(e, q, fulltext);
    found_snippet.is_some().hash(&mut v);
    let sw = s.switcher();
    (sw.mode as u8, p.theme, p.generation, p.dark, p.forced, opencode_i18n::locale().id(), e.hint.and_then(|b| b.chord())).hash(&mut v);
    let own = s.active_server();
    let session = match &e.action {
        Action::Session(id) => Some((&own, id, None)),
        Action::RemoteSession { server, id } => Some((server, id, sw.remotes.iter().find(|r| r.server == *server))),
        _ => None,
    };
    if let Action::Project { server, .. } = &e.action {
        s.avatar_memo_version().hash(&mut v);
        if let Some(proj) = project_of(s, server, &e.project) {
            (&proj.name, &proj.directory, &proj.icon.color, proj.icon.source(&proj.id)).hash(&mut v);
        }
    }
    if let Some((server, id, remote)) = session {
        let marks = s.session_marks(id);
        (server, marks.busy, marks.attention, marks.unread, s.is_session_open(id), e.updated.map(|t| relative_time(s.now_ms(), t))).hash(&mut v);
        s.avatar_memo_version().hash(&mut v);
        if let Some(remote) = remote {
            remote.session(id).map(SessionInfo::unread).hash(&mut v);
            if let Some(proj) = remote.project(&e.project) {
                (&proj.name, &proj.directory, &proj.icon.color, proj.icon.source(&proj.id)).hash(&mut v);
            }
        }
        if matches!(sw.mode, SwitcherMode::Sessions | SwitcherMode::Archived | SwitcherMode::QuickOpen) {
            s.is_session_hidden(server, id).hash(&mut v);
        }
        if let Some(r) = found_snippet {
            (r.kind, &r.snippet.text, &r.snippet.marks, r.snippet.cut_before, r.snippet.cut_after, r.more).hash(&mut v);
        }
    }
    v.finish()
}

/// A row (`PaletteRow`): 28 tall, 6 in, 6 pt corners, the hover overlay while highlighted in
/// both schemes (`dialog.css:208-215, 232-234`), its lead and text 8 apart, and its time or keys
/// at the end.
fn entry_row<H: SwitcherHost + 'static>(s: &H, p: &Pal, e: &Entry, active: bool, q: &str, w: f32, fulltext: &[FullTextRow]) -> El {
    let action = e.action.clone();
    let keyed = e.action.clone();
    let hover = e.key.clone();
    let unhide = unhide_button(s, p, e);
    let r = entry_row_shell(s, p, e, active, q, w, fulltext, unhide)
        .on_key(move |s: &mut H, ev, tx: &mut Tx| crate::row_key(s, &keyed, ev, tx))
        // Only a moving pointer takes the row (`SearchList`'s `onMouseMove`), and not `on_hover`:
        // its commit would animate the highlight's fill from row to row.
        .on_pointer_move_local(move |s: &mut H, ev: PointerEvent, tx: &mut Tx| crate::row_pointer_move(s, &hover, ev, tx))
        .on_press_tx(move |s: &mut H, tx: &mut Tx| crate::run_action(s, action.clone(), tx));
    // A row runs on the click, a release over the row it went down on (`SearchList`'s
    // `onClick`); a session's page starts loading on the press so it's there by then.
    if let Action::Session(id) = &e.action {
        let id = id.clone();
        r.on_drag(move |s: &mut H, d, _: &mut Tx| crate::row_drag(s, &id, d.phase))
    } else {
        r
    }
}

#[allow(clippy::too_many_arguments)]
fn entry_row_shell(s: &dyn SwitcherHost, p: &Pal, e: &Entry, active: bool, q: &str, w: f32, fulltext: &[FullTextRow], unhide: Option<(El, f32)>) -> El {
    let sw = s.switcher();
    let (lead, trailing) = match &e.action {
        Action::Session(id) => session_parts(s, p, e, id, s.project(&e.project), None),
        Action::RemoteSession { server, id } => {
            let remote = sw.remotes.iter().find(|r| r.server == *server);
            session_parts(s, p, e, id, remote.and_then(|r| r.project(&e.project)), remote.and_then(|r| r.session(id)))
        }
        // A project's avatar, as its sessions show it (`SessionTabAvatarView`).
        Action::Project { server, .. } => (Some(s.session_avatar(p, project_of(s, server, &e.project), &e.title, false, false)), None),
        Action::Command(_) => (None, command_keys(p, e)),
        Action::File(_) | Action::Setting(_) => (None, None),
    };
    let found_snippet = snippet(e, q, fulltext);
    let main = match &e.action {
        Action::File(path) => file_main(p, path, q),
        _ => {
            // The row's text: the row less its 8/6 pt sides, the lead and the trailing parts.
            let trailing_w = trailing.as_ref().map_or(0.0, |(_, tw)| tw + 8.0) + unhide.as_ref().map_or(0.0, |(_, uw)| uw + 8.0);
            let text_w = w - 12.0 - 14.0 - trailing_w - if lead.is_some() { 24.0 } else { 0.0 };
            let archived = sw.mode == SwitcherMode::Archived;
            let words = row_text(p, e, q, text_w, archived, stacked(s, e));
            let words = match found_snippet {
                Some(r) => col().key("found").grow(1.0).shrink().min_w(0.0).gap(0.0).child(words).child(snippet_line(p, r)),
                None => words,
            };
            row().key("main").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().children(lead).child(words)
        }
    };
    let r = row()
        .key(format!("sw-{}", e.key))
        .role("option")
        .label(e.title.clone())
        // Selected while highlighted, as official's `aria-selected` option is.
        .focus_selected(active)
        .w_full()
        .h(height(s, e, q, fulltext))
        .pl(8.0)
        .pr(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .fill(if active { p.hover } else { Color::TRANSPARENT })
        // A contrast theme's highlight is an edge, not only a wash it may lose.
        .border(if active && p.forced { 2.0 } else { 0.0 }, p.text)
        // Each row is a `<button>`, so Tab goes from the search field down the rows and wraps in
        // the dialog; one Tab reached shows the highlight's overlay and no ring, and Return or
        // Space runs it (`dialog.css:208-215`). The highlight stays where the keys left it.
        .focusable(true)
        .focus_fill(if active { Color::TRANSPARENT } else { p.hover })
        .no_focus_ring()
        .transition(Transition::none())
        .motion(Spring::NONE)
        .child(main);
    // A reader hears what matched in a session only its messages matched.
    let r = match found_snippet {
        Some(row) => r.description(row.snippet.text.clone()),
        None => r,
    };
    r.children(trailing.map(|(el, _)| el)).children(unhide.map(|(el, _)| el))
}

/// Project `id` on `server`: the window's own, or another server's the search read.
fn project_of<'a>(s: &'a dyn SwitcherHost, server: &ServerKey, id: &str) -> Option<&'a Project> {
    if *server == s.active_server() {
        return s.project(id);
    }
    s.switcher().remotes.iter().find(|r| r.server == *server).and_then(|r| r.project(id))
}

fn unhide_target(s: &dyn SwitcherHost, e: &Entry) -> Option<(ServerKey, String)> {
    if !matches!(s.switcher().mode, SwitcherMode::Sessions | SwitcherMode::Archived | SwitcherMode::QuickOpen) {
        return None;
    }
    let (server, id) = match &e.action {
        Action::Session(id) => (s.active_server(), id.clone()),
        Action::RemoteSession { server, id } => (server.clone(), id.clone()),
        _ => return None,
    };
    s.is_session_hidden(&server, &id).then_some((server, id))
}

fn unhide_button_shell(p: &Pal) -> (El, f32) {
    let label = t!("sidebar.recent.unhide");
    let w = rune::text::measure(label, &[], UI, None).0 + 16.0;
    let button = row()
        .key("unhide")
        .role("button")
        .label(label.to_owned())
        .w(w)
        .h(24.0)
        .px(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI).color(p.muted));
    (button, w)
}

/// A hidden session's Unhide, a small ghost-muted button after its row in the session search
/// (`DialogSessionSearch`'s `unhide`): it brings the session back to Recent and closes the dialog.
fn unhide_button<H: SwitcherHost + 'static>(s: &H, p: &Pal, e: &Entry) -> Option<(El, f32)> {
    let (server, id) = unhide_target(s, e)?;
    let (shell, w) = unhide_button_shell(p);
    let button = shell.on_press_tx(move |s: &mut H, tx: &mut Tx| crate::unhide_and_close(s, &server, &id, tx));
    Some((button, w))
}

/// A full text row's line of context (`UI_SMALL`, `text-faint`): what matched in the accent, an
/// ellipsis where it was cut, "Draft" before a draft's, and the session's other matches as "+n"
/// at its end.
pub fn snippet_line(p: &Pal, r: &FullTextRow) -> El {
    snippet_line_parts(p, r.kind, &r.snippet, r.more)
}

/// Renders a full-text snippet line from its individual parts so external surfaces (such as
/// Home search) can draw snippets without converting row wrappers.
pub fn snippet_line_parts(p: &Pal, kind: FullTextKind, snippet: &Snippet, more: usize) -> El {
    let lead = if snippet.cut_before { "\u{2026}" } else { "" };
    let tail = if snippet.cut_after { "\u{2026}" } else { "" };
    let body = format!("{lead}{}{tail}", snippet.text);
    let spans: Vec<Span> = snippet.marks.iter().map(|m| Span::new(m.start + lead.len(), m.end + lead.len()).color(p.accent)).collect();
    let mut line = row().key("snippet").w_full().min_w(0.0).h(16.0).gap(6.0).center_y();
    if kind == FullTextKind::Draft {
        line = line.child(text(t!("rune.search.draft")).key("kind").font(UI_SMALL).color(p.muted));
    }
    line = line.child(text(body).key("context").font(UI_SMALL).color(p.faint).spans(spans).truncate().grow(1.0).shrink().min_w(0.0).dir(Direction::Auto));
    if more > 0 {
        line = line.child(text(format!("+{more}")).key("more").font(UI_SMALL.features("\"tnum\"")).color(p.muted));
    }
    line
}

/// `command-palette-row-text`: the title (`palette-menu-title`, 13 at 530) and, 6 after it, the
/// description in `text-faint` taking what is left; with a description the title keeps at most
/// 60% (`dialog.css:217-229`). Both end in an ellipsis. An archived session's are at 70%.
fn row_text(p: &Pal, e: &Entry, q: &str, w: f32, archived: bool, stacked: bool) -> El {
    let fade = |c: Color| if archived { c.alpha(c.a * 0.7) } else { c };
    // Each in its own direction (`dialog.tsx:166-171`, `:236-243`); a project's folder reads left
    // to right, as a file's path does.
    let title = highlighted(&e.title, q, UI, fade(p.text), p.accent).truncate().dir(Direction::Auto);
    let described = if matches!(e.action, Action::Project { .. }) { Direction::Ltr } else { Direction::Auto };
    // At 640 pt or narrower the description goes under the title (`dialog.css:127-138`).
    if stacked {
        return col()
            .key("words")
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .gap(1.0)
            .child(title.shrink().min_w(0.0))
            .child(highlighted(&e.description, q, UI, fade(p.faint), p.accent).truncate().shrink().min_w(0.0).dir(described));
    }
    let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y();
    if e.description.is_empty() {
        return words.child(title.shrink().min_w(0.0));
    }
    words = words.child(row().key("title").max_w((w * 0.6).max(0.0)).center_y().child(title.shrink().min_w(0.0)));
    words.child(highlighted(&e.description, q, UI, fade(p.faint), p.accent).truncate().grow(1.0).shrink().min_w(0.0).dir(described))
}

/// A session's project avatar, with the open-tab bar 6 before it and the unread dot on its corner,
/// and its age, or the activity ring while it works (`PaletteSessionRow`).
fn session_parts(s: &dyn SwitcherHost, p: &Pal, e: &Entry, id: &str, project: Option<&Project>, remote: Option<&SessionInfo>) -> (Option<El>, Option<(El, f32)>) {
    let marks = s.session_marks(id);
    // The open session reads itself, so only a request waiting in it marks it. Another server's
    // session is unread as its server last said.
    let unread = match remote {
        Some(info) => info.unread(),
        None => !marks.busy && (marks.attention || (marks.unread && !s.is_session_open(id))),
    };
    let avatar = s.session_avatar(p, project, &e.description, e.open, unread);
    if marks.busy {
        let ring = row().key("meta").size(14.0, 16.0).center().child(activity_ring(p));
        return (Some(avatar), Some((ring, 14.0)));
    }
    let meta = e.updated.map(|at| {
        let ago = relative_time(s.now_ms(), at);
        let w = width_in(&ago, &[], META, ui_direction());
        // The full date shows on hover (`Tooltip` over `command-palette-meta`).
        let full = opencode_i18n::date::format(opencode_i18n::date::Style::MonthDayYearTime, opencode_ui::style::moment(rune::LocalTime::at(at)));
        (text(ago).key("meta").font(META).color(p.muted).help(full), w)
    });
    (Some(avatar), meta)
}

/// `getRelativeTime` (`shell/time.ts`): "Just now", "5m ago", "3h ago", "2d ago", then the local
/// date ("Nov 3", with its year when not this year's).
pub fn relative_time(now: i64, then: i64) -> String {
    let (s, m, h, d) = {
        let s = (now - then).div_euclid(1000);
        (s, s.div_euclid(60), s.div_euclid(3600), s.div_euclid(86_400))
    };
    match () {
        _ if s < 60 => t!("common.time.justNow").into(),
        _ if m < 60 => t!("common.time.minutesAgo.short", count = m),
        _ if h < 24 => t!("common.time.hoursAgo.short", count = h),
        _ if d < 7 => t!("common.time.daysAgo.short", count = d),
        _ => {
            let (at, today) = (rune::LocalTime::at(then), rune::LocalTime::at(now));
            let style = if at.year == today.year { opencode_i18n::date::Style::MonthDay } else { opencode_i18n::date::Style::MonthDayYear };
            opencode_i18n::date::format(style, opencode_ui::style::moment(at))
        }
    }
}

/// A command's chord as Desktop's neutral `Keybind` chips, and their width.
fn command_keys(p: &Pal, e: &Entry) -> Option<(El, f32)> {
    let caps = match (e.hint, e.chord) {
        (Some(bind), _) if bind.chord().is_some() => bind.keycaps(),
        (_, Some(chord)) => keymap_caps(chord),
        _ => return None,
    };
    // The chips' own label style, in their left-to-right row.
    let w = caps.iter().map(|c| width_in(&c.to_uppercase(), &[], KEYMAP_LABEL, Direction::Ltr).max(6.0) + 8.0).sum::<f32>() + 2.0 * (caps.len() as f32 - 1.0);
    Some((keymap_chips(p, caps).key("hint"), w))
}

/// A file's row: its coloured `FileIcon` badge, then its folder (`text-muted`, 440, cut first)
/// and name (`text-base`, 440) as one path (`dialog.css:91-114`).
fn file_main(p: &Pal, path: &str, q: &str) -> El {
    let (dir, name) = (opencode_kit::path::parent(path), opencode_kit::path::basename(path));
    // `getDirectory`: the folder with its slash, a bare "/" for a file at the root.
    let dir = format!("{dir}/");
    row()
        .key("main")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(8.0)
        .center_y()
        .child(icon(file_icons::file(name), 16.0).key("icon").color(p.text))
        .child(
            row()
                .key("path")
                .shrink()
                .min_w(0.0)
                .center_y()
                // A path reads left to right (`dialog.tsx:152`).
                .dir(Direction::Ltr)
                .child(highlighted(&dir, q, UI, p.muted, p.accent).truncate().shrink().min_w(0.0))
                // The name never gives way (`flex-shrink: 0`).
                .child(highlighted(name, q, UI, p.text, p.accent).min_w(width_in(name, &match_span(name, q, p.accent).into_iter().collect::<Vec<_>>(), UI, Direction::Ltr))),
        )
}
