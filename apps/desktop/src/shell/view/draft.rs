use super::*;

/// The new-session page, as Desktop's `new-session/view.tsx`: the wordmark about a quarter of
/// the way down, the composer 32 below it with where the session will start under that, and the
/// wordmark's Easter egg (`egg`) about them while it's out.
/// `width`: the panel's width last frame.
pub(crate) fn draft_page(s: &App, p: &Pal, width: f32) -> El {
    // A server that refused our credentials or runs an incompatible version covers the draft
    // (`new-session/route.tsx:42-102`); one that's away leaves it mounted and editable.
    if let Route::Draft(id) = &*s.route
        && let Some(tab) = s.tab(id)
        && let Some(cover) = super::server_unavailable::cover(&tab.server)
    {
        return col().key("draft").grow(1.0).w_full().child(super::server_unavailable::panel(p, cover, &tab.server, Some(id)));
    }
    // The wordmark's top is 25.375% down the panel (`view.tsx:105`), under the 48 pt header;
    // before the first layout, flex shares put it near there.
    let above_h = rune::memo::layout_with("draft", |r| r[3]).map(|h| (0.25375 * h - 48.0).max(0.0));
    let egg = s.kernel.slots.untracked(&crate::egg::EGG).live.is_some();
    let composer = crate::egg::view::composer_part(s, composer(s, p).max_w(880.0));
    let composer = if egg { composer.paint_order(1) } else { composer };
    let tray = crate::egg::view::composer_part(s, super::draft_location::location(s, p, width));
    let wrap = col().key("draft-composer").w_full().mt(32.0).gap(4.0).child(composer).child(tray).children(crate::egg::view::front(s));
    let content = col()
        .key("draft-content")
        .w_full()
        .max_w(880.0)
        .child(crate::egg::view::wordmark(s, p, width, above_h.map(|a| 48.0 + a), p.dark))
        .child(crate::egg::view::composer_wrap(s, wrap));
    let above = match above_h {
        Some(h) => el().key("above").h(h).shrink(),
        None => el().key("above").grow(0.3),
    };
    col()
        .key("draft")
        .grow(1.0)
        .w_full()
        .children(crate::egg::view::under(s).into_iter().flatten())
        .child(row().key("header").h(48.0).pl(14.0).center_y().window_drag().child(collapsed_slot(s)))
        .child(above)
        .child(col().key("draft-column").w_full().px(24.0).center().child(content))
        .child(el().key("below").grow(0.7))
        .children(s.diag.debug.then(|| status_bar(s, p, None)))
        .children(super::home::tip::new_session(s, p))
        .children(crate::egg::view::over(s, p))
}
