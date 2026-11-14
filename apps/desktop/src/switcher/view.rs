//! The palette drawn as Desktop's `CommandPaletteView` (`shell/commands/dialog.tsx`): the compact
//! dialog (`dialog.css:141-234`) over `SearchList` (`ui/forms/search-list/search-list.css`), its
//! rows `PaletteRow` with `palette-menu.css`'s title and `keybind.css`'s chips.

use rune::{Color, Cx, El, PointerEvent, Span, Spring, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_theme_opencode::{file_icons, icons as ic};

use super::shown::Empty;
use super::{Action, Entry, SwitcherMode};
use crate::App;
use crate::palette::{Elevate as _, Pal};
use crate::fulltext::{Row, Surface};
use crate::view::{UI, UI_MED};
use opencode_i18n::t;

/// The search block (`search-list-search` in the compact palette, `dialog.css:156-166`): 6 above
/// the 28-tall field, 17 below it, with the hairline 8 from the block's foot.
const HEAD_H: f32 = 6.0 + 28.0 + 17.0;
/// A compact row (`search-list-row[data-size=compact]`) and a section header, both 28 tall.
const ROW_H: f32 = 28.0;
/// A row whose description goes under its title in a window 640 pt or narrower (`min-height: 40px`).
const STACKED_H: f32 = 40.0;

fn entry_height(stacked: bool) -> f32 {
    if session_view::style::touch_controls() { 44.0 } else if stacked { STACKED_H } else { ROW_H }
}

/// A session only its messages matched: its title, and under it the line of context (6 above
/// and below, the title's 18 and the line's 16).
const SNIPPET_H: f32 = 46.0;

/// `e`'s row height: with its line of context, stacked, or one line.
fn height(s: &App, e: &Entry, q: &str) -> f32 {
    if snippet(s, e, q).is_some() { SNIPPET_H.max(entry_height(false)) } else { entry_height(stacked(s, e)) }
}

/// The full text row a session's palette row shows under its title: when its messages matched
/// and its title doesn't hold the query.
fn snippet(s: &App, e: &Entry, q: &str) -> Option<(std::sync::Arc<Vec<Row>>, usize)> {
    let Action::Session(id) = &e.action else { return None };
    let found = s.fulltext_row(Surface::Palette, id)?;
    let fold = opencode_search::SearchFold::for_query(q.trim());
    (!fold.fold(&e.title).contains(&fold.fold(q.trim()))).then_some(found)
}

/// Whether `e`'s row stacks its description under its title: the window is 640 pt or narrower
/// and it has one (a file's row keeps its own line).
fn stacked(s: &App, e: &Entry) -> bool {
    let w = s.layout.narrow.width();
    w > 0.0 && w <= 640.0 && !e.description.is_empty() && !matches!(e.action, Action::File(_))
}
/// Rows and the header sit 4 apart in a section, and a section ends 4 above the next
/// (`dialog.css:196-202`).
const GAP: f32 = 4.0;
/// `search-list-empty`'s height.
const EMPTY_H: f32 = 120.0;
/// `command-palette-meta`: tabular figures.
const META: TextStyle = UI.features("\"tnum\"");

/// The first match of the query in `s`, in the accent colour (`SearchHighlight`).
pub fn highlighted(s: &str, q: &str, style: TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate_fade();
    let lower = s.to_lowercase();
    let needle = q.trim().to_lowercase();
    match lower.find(&needle) {
        Some(at) if !needle.is_empty() && lower.len() == s.len() => t.spans(vec![Span::new(at, at + needle.len()).color(accent)]),
        _ => t,
    }
}

/// The scrim, and the dialog where Desktop puts it: `<Dialog size="large" fit>`
/// (`shell/commands/dialog.tsx`), as tall as its rows and growing down from the top edge.
pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let [win_w, win_h] = cx.layout_size("window").or_else(|| cx.layout_size("zoomed")).unwrap_or([1280.0, 820.0]);
    let size = crate::widgets::DialogSize::Large;
    let w = size.width(win_w);
    let scrim = el().key("switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog(s, p, w, size.height(win_h)).absolute((win_w - w) / 2.0, size.top(win_h))]
}

/// The dialog itself, `w` wide and as tall as its rows up to `max_h`.
pub fn dialog(s: &App, p: &Pal, w: f32, max_h: f32) -> El {
    let q = s.switcher.query();
    let entries = s.switcher_entries();
    let active = s.switcher_active(&entries);
    // The page pool coalesces this request. Use the row already ranked for this frame;
    // polling just to rediscover it used to run the entire search a second time.
    if let Some(id) = active.as_deref().and_then(|key| key.strip_prefix("session:")) {
        s.prefetch_session(id);
    }
    let placeholder = match s.switcher.mode {
        SwitcherMode::Sessions | SwitcherMode::Archived => t!("home.sessions.search.placeholder"),
        SwitcherMode::Commands => t!("command.commands.search"),
        // `palette.search.placeholder.home` on Home (`HomeCommandPalette`).
        _ if s.route == crate::Route::Home => t!("palette.search.placeholder.home"),
        _ => t!("palette.search.placeholder"),
    };

    // `text-input-v2` in the compact palette: 28 tall and transparent, the glyph 6 in and 6 from
    // the text, the text in `text-base` and the placeholder in it at half strength.
    // The highlighted row is the field's active descendant: arrows keep focus in the field, and
    // a reader says each row they land on (`SearchList`'s `aria-activedescendant`).
    let input = session_view::style::field(&s.switcher.search, UI)
        .key("switcher-input")
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder(placeholder, p.text.alpha(p.text.a * 0.5))
        .caret(p.text, p.selection).selection_style(p.field_selection)
        .on_edit(|s: &mut App| &mut s.switcher.search)
        .on_input_change(|s: &mut App, _: &mut Tx| s.switcher.query_changed())
        .on_input_key(|s: &mut App, ev, tx| s.switcher_nav(ev, tx) || s.shortcut(ev, tx))
        .label(placeholder);
    let input = match active.as_deref() {
        Some(k) => input.active_descendant(format!("sw-{k}")),
        None => input,
    };
    let search = row()
        .key("switcher-field")
        .h(entry_height(false))
        .pl(6.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(input);

    let mut results = col().key("switcher-results").w_full().px(6.0).pb(6.0);
    let mut content_h = 0.0_f32;
    // Only rows near the scroll view are built (`band`); the highlighted one always is.
    let window = s.switcher.band.window(max_h - HEAD_H - entry_height(false) + ROW_H);
    let mut i = 0;
    while i < entries.len() {
        let group = entries[i].group;
        let end = entries[i..].iter().position(|e| e.group != group).map_or(entries.len(), |n| i + n);
        let last = end == entries.len();
        // Space inside the section, so the next header meets this one and pushes it up. For a
        // reader the section is a group named by its header, and the header only a picture.
        let mut section = col().key(format!("sw-section-{group}")).w_full().gap(GAP).pb(if last { 0.0 } else { GAP }).role("group").label(group).child(
            row()
                .key("header")
                .w_full()
                .h(ROW_H)
                .px(6.0)
                .center_y()
                .fill(p.bg_base)
                .sticky_top(0.0)
                .a11y_hidden()
                .child(text(group).font(UI).color(p.muted)),
        );
        let size = end - i;
        let mut top = content_h + ROW_H + GAP;
        // Rows left out since the last built one: their count and heights.
        let mut skipped = (0usize, 0.0_f32);
        for (n, e) in entries[i..end].iter().enumerate() {
            let h = height(s, e, q);
            let lit = active.as_deref() == Some(e.key.as_str());
            if lit || (top < window.end && top + h > window.start) {
                if skipped.0 > 0 {
                    section = section.child(gap_for(group, n, skipped));
                    skipped = (0, 0.0);
                }
                section = section.child(entry_row(s, p, e, lit, q, w).position_in_set(n + 1, size));
            } else {
                skipped = (skipped.0 + 1, skipped.1 + h);
            }
            top += h + GAP;
        }
        if skipped.0 > 0 {
            section = section.child(gap_for(group, size, skipped));
        }
        content_h += ROW_H + entries[i..end].iter().map(|e| height(s, e, q) + GAP).sum::<f32>() + if last { 0.0 } else { GAP };
        results = results.child(section);
        i = end;
    }
    s.switcher.band.built(&window);
    if entries.is_empty() {
        match s.switcher_empty() {
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
    let header = HEAD_H + entry_height(false) - ROW_H;
    let scroll_h = content_h.min(max_h - header).max(0.0);

    col()
        .key("switcher")
        .role("dialog")
        .label(placeholder)
        .w(w)
        .h(header + scroll_h)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| s.close_switcher(true))
        .child(col().key("switcher-search").w_full().pt(6.0).px(6.0).child(search))
        // `::after`: a 0.5 hairline in `border-base` across the dialog, 8 above the block's foot.
        .child(el().key("switcher-divider").w_full().h(0.5).mt(8.5).mb(8.0).fill(p.border))
        .child(
            // `MenuScroll`: the thumb shows while the list scrolls (`search-list.tsx:311`).
            session_view::scroller::scroller("switcher-scroll").bar(session_view::scroller::Bar::MenuScroll).build::<App>(
                p,
                // `search-list.css:63-92`, `search-list.tsx:141-163`: rows fade out over 24 past the
                // bottom edge and under the stuck section header, each linear, at full strength
                // once 15 are hidden.
                col().role("listbox").w_full().h(scroll_h).scroll_fade(rune::ScrollFade::strength(24.0).linear().below_sticky()).on_scroll_offset(s.switcher.band.watch((*s.waker).clone())),
                results,
            ),
        )
}

/// `count` rows left unbuilt before row `n` of `group`, as tall as they and the gaps between them.
fn gap_for(group: &str, n: usize, (count, h): (usize, f32)) -> El {
    el().key(format!("sw-gap-{group}-{n}")).w_full().h(h + (count - 1) as f32 * GAP).a11y_hidden()
}

/// A row (`PaletteRow`): 28 tall, 6 in, 6 pt corners, the hover overlay while highlighted in
/// both schemes (`dialog.css:208-215, 232-234`), its lead and text 8 apart, and its time or keys
/// at the end.
fn entry_row(s: &App, p: &Pal, e: &Entry, active: bool, q: &str, w: f32) -> El {
    let action = e.action.clone();
    let keyed = e.action.clone();
    let hover = e.key.clone();
    let (lead, trailing) = match &e.action {
        Action::Session(id) => session_parts(s, p, e, id, s.project(&e.project), None),
        Action::RemoteSession { server, id } => {
            let remote = s.switcher.remotes.iter().find(|r| r.server == *server);
            session_parts(s, p, e, id, remote.and_then(|r| r.project(&e.project)), remote.and_then(|r| r.session(id)))
        }
        Action::Command(_) => (None, command_keys(p, e)),
        Action::File(_) | Action::Setting(_) => (None, None),
    };
    let unhide = unhide_button(s, p, e);
    let main = match &e.action {
        Action::File(path) => file_main(p, path, q),
        _ => {
            // The row's text: the row less its 6 pt sides, the lead and the trailing parts.
            let trailing_w = trailing.as_ref().map_or(0.0, |(_, w)| w + 8.0) + unhide.as_ref().map_or(0.0, |(_, w)| w + 8.0);
            let text_w = w - 12.0 - 12.0 - trailing_w - if lead.is_some() { 24.0 } else { 0.0 };
            let archived = s.switcher.mode == SwitcherMode::Archived;
            let words = row_text(p, e, q, text_w, archived, stacked(s, e));
            let words = match snippet(s, e, q) {
                Some((rows, at)) => col().key("found").grow(1.0).shrink().min_w(0.0).gap(0.0).child(words).child(snippet_line(p, &rows[at])),
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
        .h(height(s, e, q))
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if active { p.hover } else { Color::TRANSPARENT })
        // A contrast theme's highlight is an edge, not only a wash it may lose.
        .border(if active && p.forced { 2.0 } else { 0.0 }, p.text)
        // Each row is a `<button>`, so Tab goes from the search field down the rows and wraps in
        // the dialog; one Tab reached shows the highlight's overlay and no ring, and Return or
        // Space runs it (`dialog.css:208-215`). The highlight stays where the keys left it.
        .focusable(true)
        .focus_fill(if active { Color::TRANSPARENT } else { p.hover })
        .no_focus_ring()
        .on_key(move |s: &mut App, ev, tx: &mut Tx| {
            let press = matches!(ev.key, rune::Key::Enter) || ev.key == rune::Key::Text(" ".into());
            if press && !ev.mods.cmd && !ev.mods.alt && !ev.mods.shift {
                s.switcher_run(keyed.clone(), tx);
            }
            press
        })
        .transition(Transition::none())
        .motion(Spring::NONE)
        // Only a moving pointer takes the row (`SearchList`'s `onMouseMove`), and not `on_hover`:
        // its commit would animate the highlight's fill from row to row.
        .on_pointer_move_local(move |s: &mut App, ev: PointerEvent, tx: &mut Tx| {
            if !s.switcher.point(&hover, [ev.window_x, ev.window_y]) {
                return tx.live_only();
            }
            tx.instant();
            if let Some(id) = hover.strip_prefix("session:") { s.prefetch_session(id); }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.switcher_run(action.clone(), tx))
        .child(main);
    // A reader hears what matched in a session only its messages matched.
    let r = match snippet(s, e, q) {
        Some((rows, at)) => r.description(rows[at].snippet.text.clone()),
        None => r,
    };
    // A row runs on the click, a release over the row it went down on (`SearchList`'s
    // `onClick`); a session's page starts loading on the press so it's there by then.
    let r = if let Action::Session(id) = &e.action {
        let id = id.clone();
        r.on_drag(move |s: &mut App, d, _: &mut Tx| {
            if d.phase == rune::Phase::Down {
                s.prefetch_session(&id);
            }
        })
    } else { r };
    r.children(trailing.map(|(el, _)| el)).children(unhide.map(|(el, _)| el))
}

/// A hidden session's Unhide, a small ghost-muted button after its row in the session search
/// (`DialogSessionSearch`'s `unhide`): it brings the session back to Recent and closes the dialog.
fn unhide_button(s: &App, p: &Pal, e: &Entry) -> Option<(El, f32)> {
    if !matches!(s.switcher.mode, SwitcherMode::Sessions | SwitcherMode::Archived) {
        return None;
    }
    let (server, id) = match &e.action {
        Action::Session(id) => (crate::servers::runtime::active(s).clone(), id.clone()),
        Action::RemoteSession { server, id } => (server.clone(), id.clone()),
        _ => return None,
    };
    if !crate::session_menu::preferences::hidden(&server, &id) {
        return None;
    }
    let label = t!("sidebar.recent.unhide");
    let w = rune::text::measure(label, &[], UI, None).0 + 16.0;
    let button = row().key("unhide").role("button").label(label.to_owned()).w(w).h(24.0).px(8.0).center_y().radius(6.0).hover(p.hover).press(p.pressed)
        .child(text(label.to_owned()).font(UI).color(p.muted))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            crate::session_menu::preferences::update(s, crate::session_menu::preferences::Kind::Hidden, &server, &id, false);
            s.close_switcher(true);
        });
    Some((button, w))
}

/// A full text row's line of context (`UI_SMALL`, `text-faint`): what matched in the accent, an
/// ellipsis where it was cut, "Draft" before a draft's, and the session's other matches as "+n"
/// at its end.
pub(crate) fn snippet_line(p: &Pal, r: &Row) -> El {
    let small = session_view::style::UI_SMALL;
    let lead = if r.snippet.cut_before { "\u{2026}" } else { "" };
    let tail = if r.snippet.cut_after { "\u{2026}" } else { "" };
    let body = format!("{lead}{}{tail}", r.snippet.text);
    let spans: Vec<Span> = r.snippet.marks.iter().map(|m| Span::new(m.start + lead.len(), m.end + lead.len()).color(p.accent)).collect();
    let mut line = row().key("snippet").w_full().min_w(0.0).h(16.0).gap(6.0).center_y();
    if r.kind == opencode_search::fulltext::Kind::Draft {
        line = line.child(text(t!("rune.search.draft")).key("kind").font(small).color(p.muted));
    }
    line = line.child(text(body).key("context").font(small).color(p.faint).spans(spans).truncate().grow(1.0).shrink().min_w(0.0).dir(rune::Direction::Auto));
    if r.more > 0 {
        line = line.child(text(format!("+{}", r.more)).key("more").font(small.features("\"tnum\"")).color(p.muted));
    }
    line
}

/// `command-palette-row-text`: the title (`palette-menu-title`, 13 at 530) and, 6 after it, the
/// description in `text-faint` taking what is left; with a description the title keeps at most
/// 60% (`dialog.css:217-229`). Both end in an ellipsis. An archived session's are at 70%.
fn row_text(p: &Pal, e: &Entry, q: &str, w: f32, archived: bool, stacked: bool) -> El {
    let fade = |c: Color| if archived { c.alpha(c.a * 0.7) } else { c };
    // Each in its own direction (`dialog.tsx:166-171`, `:236-243`).
    let title = highlighted(&e.title, q, UI_MED, fade(p.text), p.accent).truncate().dir(rune::Direction::Auto);
    // At 640 pt or narrower the description goes under the title (`dialog.css:127-138`).
    if stacked {
        return col().key("words").grow(1.0).shrink().min_w(0.0).gap(1.0)
            .child(title.shrink().min_w(0.0))
            .child(highlighted(&e.description, q, UI, fade(p.faint), p.accent).truncate().shrink().min_w(0.0).dir(rune::Direction::Auto));
    }
    let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y();
    if e.description.is_empty() {
        return words.child(title.shrink().min_w(0.0));
    }
    words = words.child(row().key("title").max_w((w * 0.6).max(0.0)).center_y().child(title.shrink().min_w(0.0)));
    words.child(highlighted(&e.description, q, UI, fade(p.faint), p.accent).truncate().grow(1.0).shrink().min_w(0.0).dir(rune::Direction::Auto))
}

/// A session's project avatar, with the open-tab bar 6 before it and the unread dot on its corner,
/// and its age, or the activity ring while it works (`PaletteSessionRow`).
fn session_parts(s: &App, p: &Pal, e: &Entry, id: &str, project: Option<&opencode_kit::model::Project>, remote: Option<&opencode_kit::model::SessionInfo>) -> (Option<El>, Option<(El, f32)>) {
    let marks = s.marks(id);
    // The open session reads itself, so only a request waiting in it marks it. Another server's
    // session is unread as its server last said.
    let unread = match remote {
        Some(info) => info.unread(),
        None => !marks.busy && (marks.attention || (marks.unread && s.route != crate::Route::here(id.to_owned()))),
    };
    let avatar = crate::icons::session_avatar(s, p, project, &e.description, e.open, unread);
    if marks.busy {
        let ring = row().key("meta").size(14.0, 16.0).center().child(session_view::style::activity_ring(p));
        return (Some(avatar), Some((ring, 14.0)));
    }
    let meta = e.updated.map(|at| {
        let ago = relative_time(*s.now_ms, at);
        let w = rune::text::measure(&ago, &[], META, None).0;
        // The full date shows on hover (`Tooltip` over `command-palette-meta`).
        let full = opencode_i18n::date::format(opencode_i18n::date::Style::MonthDayYearTime, session_view::turn_errors::moment(rune::LocalTime::at(at)));
        (text(ago).key("meta").font(META).color(p.muted).help(full), w)
    });
    (Some(avatar), meta)
}

/// `getRelativeTime` (`shell/time.ts`): "Just now", "5m ago", "3h ago", "2d ago", then the local
/// date ("Nov 3", with its year when not this year's).
pub(crate) fn relative_time(now: i64, then: i64) -> String {
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
            opencode_i18n::date::format(style, session_view::turn_errors::moment(at))
        }
    }
}

/// A command's chord as Desktop's neutral `Keybind` chips, and their width.
fn command_keys(p: &Pal, e: &Entry) -> Option<(El, f32)> {
    let caps = match (e.hint, e.chord) {
        (Some(bind), _) if bind.chord().is_some() => bind.keycaps(),
        (_, Some(chord)) => crate::keymap::caps(chord),
        _ => return None,
    };
    let label = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05);
    let w = caps.iter().map(|c| rune::text::measure(&c.to_uppercase(), &[], label, None).0.max(6.0) + 8.0).sum::<f32>() + 2.0 * (caps.len() as f32 - 1.0);
    Some((crate::keymap::chips(p, caps).key("hint"), w))
}

/// A file's row: its coloured `FileIcon` badge, then its folder (`text-muted`, 440, cut first)
/// and name (`text-base`, 530) as one path (`dialog.css:91-114`).
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
                .dir(rune::Direction::Ltr)
                .child(highlighted(&dir, q, UI, p.muted, p.accent).truncate().shrink().min_w(0.0))
                // The name never gives way (`flex-shrink: 0`).
                .child(highlighted(name, q, UI_MED, p.text, p.accent).min_w(rune::text::measure(name, &[], UI_MED, None).0)),
        )
}
