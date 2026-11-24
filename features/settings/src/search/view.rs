//! Rail search input and result list views for Settings search (`settings/search.tsx`, `search.css`).

use opencode_i18n::t;
use opencode_ui::focus::FocusExt;
use opencode_ui::palette::Pal;
use rune::{Cursor, El, Key, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{Entry, active_index, category, category_label, clear, open, shown};
use crate::host::SettingsHost;
use crate::pages;

/// The results' labels and rows: 13 on 16 at 440, -0.04 tracking.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// The field's text below 800 pt: 16 on `line-height-compact`'s 16.
const NARROW_INPUT: TextStyle = TextStyle::new(16.0, 440.0).lh(16.0).tracking(-0.04);

fn clear_and_focus(s: &mut dyn SettingsHost, tx: &mut Tx) {
    tx.instant();
    clear(s);
    s.settings_pages_mut().find.query.focused = true;
}

fn submit_search(s: &mut dyn SettingsHost, tx: &mut Tx) {
    let (list, _) = shown(s);
    let idx = active_index(&list, s.settings_pages().find.highlighted.as_deref());
    if let Some(e) = list.get(idx).cloned() {
        open(s, &e, tx);
    }
}

fn input_changed(s: &mut dyn SettingsHost, tx: &mut Tx) {
    tx.instant();
    let find = &mut s.settings_pages_mut().find;
    find.highlighted = None;
    find.collapsed_narrow = false;
    tx.scroll_to_instant("settings-results-scroll", 0.0, 0.0);
}

fn hover_result(s: &mut dyn SettingsHost, id: &str, inside: bool, tx: &mut Tx) {
    if inside && s.settings_pages().find.highlighted.as_deref() != Some(id) {
        tx.instant();
        s.settings_pages_mut().find.highlighted = Some(id.to_owned());
    }
}

/// The rail's field (`.settings-search`): 28 pt, transparent until hovered, focused or typed in.
pub fn field<H: SettingsHost + 'static>(s: &H, p: &Pal, narrow: bool) -> El {
    let q = &s.settings_pages().find.query;
    let lit = q.focused || !q.text.is_empty();
    let clear_button = (!q.text.is_empty()).then(|| {
        row()
            .key("settings-search-clear")
            .role("button")
            .label_static(t!("common.clear"))
            .size(20.0, 20.0)
            .center()
            .radius(4.0)
            .cursor(Cursor::Pointer)
            .focus_default(p)
            .hover_group_focus()
            .on_press_tx(|s: &mut H, tx: &mut Tx| clear_and_focus(s, tx))
            .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_faint).group_color(p.icon))
    });
    let field = row()
        .key("settings-search")
        .w_full()
        .h(28.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .fill(if lit { p.hover } else { rune::Color::TRANSPARENT })
        .hover(p.hover);
    let field = if q.focused { field.border(1.0, p.v2.border_focus) } else { field };
    field
        // `search.css`: the page rows below inset their icons 6; the other 2 goes after the icon,
        // so the text still starts where it did.
        .child(el().pl(6.0).pr(2.0).child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted)))
        .child(
            opencode_ui::style::field(q, if narrow { NARROW_INPUT } else { ROW })
                .key("settings-search-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(t!("settings.search.placeholder"), p.faint)
                .placeholder_font(if narrow && !opencode_ui::style::touch_controls() { NARROW_INPUT } else { ROW })
                .caret(p.text, p.selection)
                .selection_style(p.field_selection)
                .label_static(t!("settings.search.placeholder"))
                .on_edit(|s: &mut H| &mut s.settings_pages_mut().find.query)
                .on_input_key(|s: &mut H, ev, tx| ev.key == Key::Escape && crate::settings_key(s, ev, tx))
                .on_submit(|s: &mut H, tx: &mut Tx| submit_search(s, tx))
                .on_input_change(|s: &mut H, tx: &mut Tx| input_changed(s, tx)),
        )
        .children(clear_button)
}

/// The results in the rail (`SearchList`): sections under sticky 28 pt headers, 36 pt rows 1 apart.
pub fn results<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let (list, more) = shown(s);
    let query = s.settings_pages().find.query.text.trim().to_owned();
    if list.is_empty() {
        return row()
            .key("settings-results")
            .w_full()
            .h(120.0)
            .center()
            .child(pages::no_results(p, "settings-search-empty", &query));
    }
    let active = active_index(&list, s.settings_pages().find.highlighted.as_deref());
    let active_fill = if p.dark { rune::Color::WHITE.alpha(0.10) } else { p.hover };
    let mut body = col().key("settings-results-list").w_full().pt(6.0).px(6.0).pb(8.0);
    let mut at = 0;
    let groups: Vec<&'static str> = list.iter().map(category).fold(Vec::new(), |mut v, c| {
        if !v.contains(&c) {
            v.push(c);
        }
        v
    });
    for group in groups {
        let entries: Vec<&Entry> = list.iter().filter(|e| category(e) == group).collect();
        let icons = entries.iter().any(|e| e.top || e.project.is_some());
        let mut section = col()
            .key(format!("settings-results-{group}"))
            .w_full()
            .gap(1.0)
            .pb(16.0)
            .child(
                row()
                    .key("header")
                    .w_full()
                    .h(28.0)
                    .mx(-6.0)
                    .px(18.0)
                    .center_y()
                    .fill(p.bg_base)
                    .sticky_top(0.0)
                    .child(text(category_label(group)).font(ROW).color(p.muted)),
            );
        for e in entries {
            let entry = e.clone();
            let id = e.id.clone();
            let mut r = row()
                .key(format!("settings-result-{at}"))
                .role("option")
                .label(e.title.clone())
                .focus_selected(at == active)
                .cursor(Cursor::Pointer)
                .w_full()
                .h(36.0)
                .px(12.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .on_hover_change(move |s: &mut H, inside, tx: &mut Tx| hover_result(s, &id, inside, tx))
                .on_press_tx(move |s: &mut H, tx: &mut Tx| open(s, &entry, tx));
            if !e.where_.is_empty() {
                r = r.help(e.where_.clone());
            }
            if at == active {
                r = r.fill(active_fill);
            }
            if icons {
                let mark = match &e.project {
                    Some(dir) => s.project_badge(p, s.project_at(dir), &e.title),
                    None => icon(e.icon, 16.0).color(p.icon_muted),
                };
                r = r.child(el().size(16.0, 16.0).center().child(mark));
            }
            r = r.child(highlight(p, &e.title, &query));
            if e.where_ != e.title && !(e.entity && e.project.is_some()) {
                r = r.child(text(e.where_.clone()).font(ROW).color(p.muted).shrink().min_w(0.0).truncate());
            }
            section = section.child(r);
            at += 1;
        }
        body = body.child(section);
    }
    let footer = more.then(|| {
        row()
            .key("settings-results-footer")
            .w_full()
            .pt(4.0)
            .px(18.0)
            .pb(12.0)
            .child(text(t!("settings.search.refine")).font(ROW).color(p.muted).wrap())
    });
    col()
        .key("settings-results")
        .w_full()
        .min_h(0.0)
        .child(
            opencode_ui::scroller::scroller("settings-results-scroll")
                .bar(opencode_ui::scroller::Bar::MenuScroll)
                .build::<H>(p, col().w_full().shrink().min_h(0.0).clip_fade(0.0, 0.0, 0.0, 24.0), body),
        )
        .children(footer)
}

/// A title with the query's first match in the accent (`SearchHighlight`).
fn highlight(p: &Pal, title: &str, query: &str) -> El {
    let t = text(title.to_owned()).font(ROW).color(p.text).truncate().shrink().min_w(0.0);
    let fold = rune_kit::search::Fold::for_language(opencode_i18n::locale().tag());
    match fold.find(title, query.trim()).first() {
        Some(r) => t.spans(vec![rune::Span::new(r.start, r.end).color(p.accent)]),
        None => t,
    }
}
