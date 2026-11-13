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
    let egg = s.egg.live.is_some();
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
        .children(s.debug.then(|| status_bar(s, p, None)))
        .children(crate::egg::view::over(s, p))
}

/// The logo's blocks at `k` points a unit (234 x 42 units): letters in `stem`, the counters'
/// lower halves in `weak`.
pub(crate) fn logo_at(k: f32, stem: rune::Color, weak: rune::Color) -> El {
    // x, y, w, h in the logo's 234x42 units; `true` for a counter's lower half.
    const RECTS: [(f32, f32, f32, f32, bool); 40] = [
        (0., 6., 24., 6., false), (0., 12., 6., 24., false), (18., 12., 6., 24., false), (6., 30., 12., 6., false), (6., 18., 12., 12., true),
        (30., 6., 24., 6., false), (30., 12., 6., 30., false), (48., 12., 6., 24., false), (36., 30., 12., 6., false), (36., 18., 12., 12., true),
        (60., 6., 24., 6., false), (60., 12., 6., 24., false), (78., 12., 6., 12., false), (66., 18., 12., 6., false), (66., 30., 18., 6., false), (66., 24., 18., 6., true),
        (90., 6., 18., 6., false), (90., 12., 6., 24., false), (108., 12., 6., 24., false), (96., 18., 12., 18., true),
        (120., 6., 24., 6., false), (120., 12., 6., 18., false), (120., 30., 24., 6., false), (126., 18., 18., 12., true),
        (150., 6., 24., 6., false), (150., 12., 6., 24., false), (168., 12., 6., 24., false), (156., 30., 12., 6., false), (156., 18., 12., 12., true),
        (180., 6., 18., 6., false), (198., 0., 6., 36., false), (180., 12., 6., 24., false), (186., 30., 12., 6., false), (186., 18., 12., 12., true),
        (210., 6., 24., 6., false), (210., 12., 6., 24., false), (228., 12., 6., 12., false), (216., 18., 12., 6., false), (216., 30., 18., 6., false), (216., 24., 18., 6., true),
    ];
    // A picture, as Desktop's SVG and canvas are: right to left doesn't mirror it.
    let mut logo = el().size(78.0 * 3.0 * k, 14.0 * 3.0 * k).dir(rune::Direction::Ltr);
    for (x, y, w, h, counter) in RECTS {
        logo = logo.child(el().absolute(x * k, y * k).size(w * k, h * k).fill(if counter { weak } else { stem }));
    }
    logo
}
