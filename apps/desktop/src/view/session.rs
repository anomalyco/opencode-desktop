use super::*;

#[allow(unused_imports)]
pub(crate) use session_view::timeline::{
    BUBBLE_MAX, EAGER_ROWS, Place, ROW_BUILDS, RowContent, RowSpec, divider_runs, interrupted,
    joins, markdown_style, prompts_join, readable, row_keys, timeline, timeline_reset_row_builds,
    timeline_row_builds,
};

#[cfg(test)]
pub(in crate::view) fn markdown_view(p: &Pal, src: &str, fade: f32, streaming: bool) -> El {
    session_view::timeline::markdown_view::<App>(p, src, fade, streaming)
}

pub(crate) fn markdown_tinted(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    session_view::style::markdown_tinted::<App>(p, src, fade, streaming, color, width)
}

// ---------------------------------------------------------------------------------------------
// Session

pub(crate) fn session_page(s: &App, p: &Pal, id: &str) -> El {
    let shown = s.displayed_session(id).unwrap_or(id);
    let t = s.transcripts.get(shown).filter(|_| !s.loading_session(id));
    let head = header(s, p, s.session(shown), t);
    let page = col().key("session").grow(1.0).w_full().child(if s.opening.is_some() { head.inert() } else { head });
    session_body(s, p, id, page)
}

/// The session page under its header: the timeline, and what sits under it.
pub(super) fn session_body(s: &App, p: &Pal, id: &str, page: El) -> El {
    let info = s.session(id);
    let shown = s.displayed_session(id);
    let t = shown.and_then(|shown| s.transcripts.get(shown)).filter(|_| !s.loading_session(id));
    let child = info.is_some_and(|i| i.parent.is_some());
    let pills = if child { Vec::new() } else { pills(s, p, id) };
    let timeline = match shown {
        Some(shown) => session_view::timeline_area(s, p, shown, t, pills),
        None => col().key("timeline-area").grow(1.0).w_full(),
    };
    let mut page = page
        .child(if s.opening.is_some() { timeline.inert() } else { timeline })
        .child(composer_dock(s, p));
    page = page.children(s.debug.then(|| status_bar(s, p, info)));
    page
}

/// The pills over the composer: the subagent capsule, then the todo pill beside it.
fn pills(s: &App, p: &Pal, id: &str) -> Vec<El> {
    // Background activity's Hidden takes the running work's capsule away (`activity-chips.tsx`).
    let capsule = (s.settings.background_activity != crate::settings::BackgroundActivity::Off).then(|| crate::subagents::capsule(s, p, id)).flatten();
    let gap = if capsule.is_some() { crate::jump::GAP } else { 0.0 };
    // The timeline's edge rises over their row first, so a pill fades in only once nothing is
    // under it, and goes at once so the edge can come back down (t1129).
    let enter = |pill: El| pill.transition(rune::Transition::asymmetric(rune::Transition::opacity(), rune::Transition::none())).enter_delay(PILL_WAIT);
    let own: Vec<El> = capsule.into_iter().chain(crate::todos::pill(s, p, id).map(|t| t.ml(gap))).collect();
    // Plugins' pills follow the app's (`SessionPluginPills`), 8 apart.
    let gap = if own.is_empty() { 0.0 } else { 8.0 };
    let plugins = crate::plugin_host::pill_views(s, p, id).into_iter().enumerate().map(|(k, pill)| pill.ml(if k == 0 { gap } else { 8.0 }));
    own.into_iter().chain(plugins).map(enter).collect()
}

/// How long the timeline's edge takes to clear the pills' row (the page's spring).
const PILL_WAIT: f32 = 0.3;

/// The storybook's stages (`stories.rs`): the open session's timeline and what sits under it,
/// its header, and its composer with the subagent capsule over it.
pub(crate) fn story_session(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return el() };
    session_body(s, p, id, col().key("session").grow(1.0).w_full().min_h(0.0))
}

pub(crate) fn story_header(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return el() };
    header(s, p, s.session(id), s.transcripts.get(id))
}

pub(crate) fn story_composer(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return composer_dock(s, p) };
    let pills = pills(s, p, id);
    col().key("session")
        .w_full()
        .children((!pills.is_empty()).then(|| col().key("activity").w_full().center().px(DOCK_SIDE).pb(8.0)
            .child(row().key("activity-lane").w_full().max_w(session_view::style::dock_w()).children(pills))))
        .child(composer_dock(s, p))
}
