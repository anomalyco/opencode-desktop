use super::*;

// ---------------------------------------------------------------------------------------------
// Home's session search (`HomeSessionSearch` in `home/sessions/view.tsx`): the field, and while
// it has focus and text, its results in a floating panel that holds the field 6 in from its top.

/// `h-9 rounded-[6px] py-1 ps-3 pe-2 gap-2`, `bg-layer-02/60`, `bg-layer-02` hovered or focused.
pub(super) fn field(s: &App, p: &Pal, selected: Option<&model::Project>, width: f32, window_h: f32, narrow: bool) -> El {
    let open = s.home_search_open();
    let placeholder = placeholder(s, selected);
    // Not `search`: that key is the titlebar's and the sidebar's Search sessions button.
    let mut field = row()
        .key("home-search")
        .w_full()
        .h(36.0)
        .pl(12.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if s.search.focused { p.layer2 } else { p.search_bg })
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            // The input's 24 line (13/24, `home-sessions`), which its selection fills.
            session_view::style::field(&s.search, UI.lh(24.0))
                .key("search-input")
                // Named by its placeholder, which goes once a letter is typed.
                .label(placeholder.to_owned())
                .spellcheck(s.settings.spellcheck)
                .single_line()
                .grow(1.0)
                .shrink()
                .h(24.0)
                .color(p.text)
                .placeholder(placeholder, p.faint)
                .caret(p.text, p.selection).selection_style(p.field_selection)
                .on_edit(|s: &mut App| &mut s.search)
                .on_input_change(|s: &mut App, _: &mut Tx| s.home_search.query_changed())
                .on_input_key(|s: &mut App, ev, tx| s.home_search_key(ev, tx)),
        );
    if !s.search.text.is_empty() {
        // A ghost-muted small IconButton (20, `rounded-[4px]`) with the large `close` icon.
        field = field.child(
            row()
                .key("search-clear")
                .role("button")
                .label_static(opencode_i18n::t!("common.clear"))
                .size(20.0, 20.0)
                .center()
                .radius(4.0)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.clear_home_search(true, tx))
                .child(icon(&ic::CLOSE, 20.0).color(p.icon_muted)),
        );
    }
    let root = col().key("home-search-root").w_full();
    if !open {
        return root.child(field);
    }
    root.child(panel(s, p, selected, width, window_h, narrow)).child(field)
}

/// `search.ts:101-113`: "Search sessions in <project>" for a selected project; with several
/// servers, "Search sessions in <server>"; else "Search sessions".
fn placeholder(s: &App, selected: Option<&model::Project>) -> String {
    if let Some(proj) = selected {
        return opencode_i18n::t!("home.sessions.search.placeholder.scoped", scope = proj.name);
    }
    let key = crate::projects::home::server(s);
    match s.home_state.servers.iter().find(|v| v.key == key).filter(|_| s.home_state.servers.len() > 1) {
        Some(v) => opencode_i18n::t!("home.sessions.search.placeholder.scoped", scope = v.label),
        None => opencode_i18n::t!("home.sessions.search.placeholder").into(),
    }
}

/// The results panel: `rounded-[12px] bg-base`, the floating elevation, 6 out from the field on
/// every side but the bottom; `pt-9` keeps the field's room, then `pt-4` before the results.
fn panel(s: &App, p: &Pal, selected: Option<&model::Project>, width: f32, window_h: f32, narrow: bool) -> El {
    let results = s.home_search_results(selected);
    let ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
    let active = s.home_search_active(&ids);
    let show_project = selected.is_none();
    // A result: `pl-[18px] pr-6`, the avatar and `gap-1.5`, inside the panel 12 wider than the field.
    let labels_w = (width + 12.0 - 18.0 - 24.0 - 16.0 - if cfg!(target_arch = "wasm32") { 8.0 } else { 6.0 }).max(0.0);
    let body = if s.home_search_loading() {
        // `px-4 py-3`, a 16 pt Loader.
        row().key("home-search-loading").w_full().px(16.0).py(12.0).center().child(activity_ring_sized(p, 16.0))
    } else if results.is_empty() {
        // `my-1.5 px-4 pb-2`, 13/16 muted.
        row()
            .key("home-search-empty")
            .w_full()
            .px(16.0)
            .pb(8.0)
            .my(6.0)
            .child(text(opencode_i18n::t!("home.sessions.search.noResults", query = s.search.text.trim())).font(UI).color(p.muted).truncate())
    } else {
        let q = s.search.text.trim();
        let fold = opencode_search::SearchFold::for_query(q);
        let rows = col()
            .key("home-search-rows")
            .w_full()
            .gap(1.0)
            .pb(8.0)
            .children(results.iter().map(|ses| {
                // A session only its messages matched shows the line of context under its row.
                let found = s.fulltext_row(crate::fulltext::Surface::Home, &ses.id).filter(|_| !fold.fold(&ses.title).contains(&fold.fold(q)));
                let r = result_row(s, p, ses, active.as_deref() == Some(ses.id.as_str()), show_project, labels_w, narrow);
                let Some((rows, at)) = found else { return r };
                // A reader hears what matched with the row; the line itself is a picture of it.
                let r = r.description(rows[at].snippet.text.clone());
                let id = ses.id.clone();
                let line = row()
                    .key("srf-line")
                    .w_full()
                    .pl(18.0 + 22.0)
                    .pr(24.0)
                    .pb(6.0)
                    .a11y_hidden()
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_home_result(&id, tx))
                    .child(crate::switcher::view::snippet_line(p, &rows[at]));
                col().key(format!("srf-{}", ses.id)).w_full().child(r).child(line)
            }));
        col()
            .key("home-search-results")
            .w_full()
            // `my-1.5 pl-[18px] pr-6`.
            .child(row().key("home-search-label").w_full().pl(18.0).pr(24.0).my(6.0).child(text(opencode_i18n::t!("home.sessions.search.sessions")).font(UI).color(p.muted)))
            // `max-h-[min(20rem,40dvh)]`.
            // `MenuScroll` (`home/sessions/view.tsx:459`): its thumb shows while the list scrolls.
            .child(
                session_view::scroller::scroller("home-search-list").bar(session_view::scroller::Bar::MenuScroll).build::<App>(
                    p,
                    col().w_full().max_h(320.0f32.min(window_h * 0.4)).clip_soft(0.0),
                    rows,
                ),
            )
    };
    col()
        .key("home-search-panel")
        .role("listbox")
        .pin(Some(-6.0), Some(-6.0), Some(-6.0), None)
        .pt(36.0 + 16.0)
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.floating)
        .transition(rune::Transition::none())
        .motion(rune::Spring::NONE)
        // Presses in the panel keep the field focused, so it stays open until a result opens.
        .keeps_text_selection()
        .child(body)
}

/// `HomeSessionSearchResultRow`: 40 tall, `pl-[18px] pr-6`, the overlay when hovered or
/// highlighted; the pointer highlights it.
fn result_row(s: &App, p: &Pal, ses: &SessionInfo, selected: bool, show_project: bool, labels_w: f32, stacked: bool) -> El {
    let parts = Parts::of(s, p, ses, labels_w, show_project, stacked, 6.0);
    let (id, hover_id) = (ses.id.clone(), ses.id.clone());
    let mut r = row()
        .key(format!("sr-{}", ses.id))
        .role("option")
        .label(ses.title.clone())
        .w_full()
        .h(if stacked && show_project { 48.0 } else { 40.0 })
        .pl(18.0)
        .pr(24.0)
        .gap(if cfg!(target_arch = "wasm32") { 8.0 } else { 6.0 })
        .center_y()
        .hover_group()
        .hover(p.hover)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                s.home_search.activate(hover_id.clone());
            }
            super::hover_session(s, &hover_id, inside, tx);
            if inside {
                tx.instant();
            }
        })
        .pointer_buttons([rune::PointerButton::Middle])
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if s.session(&id).is_none()
                && let Some(ses) = s.home_search.search.exact().filter(|x| x.id == id).cloned()
            {
                s.sessions.push(ses);
            }
            // In the background the search stays open (`search.ts` `select`).
            if super::opens_in_background(tx) {
                return s.open_in_background(&id);
            }
            s.open_home_result(&id, tx);
        })
        .child(parts.avatar)
        .child(parts.labels)
        .children(parts.ring);
    if selected {
        r = r.fill(p.hover);
    }
    r
}
