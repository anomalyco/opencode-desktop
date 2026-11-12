use super::*;

/// The new-session page, as Desktop's `new-session/view.tsx`: the wordmark about a quarter of
/// the way down, the composer 32 below it with where the session will start under that.
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
    let content = col()
        .key("draft-content")
        .w_full()
        .max_w(880.0)
        .child(wordmark(p, width))
        .child(col().key("draft-composer").w_full().mt(32.0).gap(4.0).child(composer(s, p).max_w(880.0)).child(super::draft_location::location(s, p, width)));
    // The wordmark's top is 25.375% down the panel (`view.tsx:105`), under the 48 pt header;
    // before the first layout, flex shares put it near there.
    let above = match rune::memo::layout_with("draft", |r| r[3]) {
        Some(h) => el().key("above").h((0.25375 * h - 48.0).max(0.0)).shrink(),
        None => el().key("above").grow(0.3),
    };
    col()
        .key("draft")
        .grow(1.0)
        .w_full()
        .child(row().key("header").h(48.0).pl(14.0).center_y().window_drag().child(collapsed_slot(s)))
        .child(above)
        .child(col().key("draft-column").w_full().px(24.0).center().child(content))
        .child(el().key("below").grow(0.7))
        .children(s.debug.then(|| status_bar(s, p, None)))
}

/// OpenCode's wordmark (`ui/logo.tsx`) as Desktop's new-session page draws it at rest: every
/// letter in `--icon-base` at 16%, the counters in 30% of it (`wordmark.tsx:240-242`,
/// `wordmark.css:44-46`). Its box spans 80% of up to 720 pt at the logo's 234:42
/// (`wordmark.tsx:170-178`); the logo inside is 78 × 14 whole-point pitches, the pitch
/// `max(3, round(box / 78))`, centred in it (`fitStage`, `wordmark.tsx:410-428`).
pub(in crate::view) fn wordmark(p: &Pal, width: f32) -> El {
    let span = 0.8 * if width > 0.0 { (width - 48.0).min(720.0) } else { 720.0 };
    let pitch = (span / 78.0).round().max(3.0);
    // A pitch is 3 of the logo's units.
    let k = pitch / 3.0;
    let icon = p.standard.icon;
    let (stem, weak) = (icon.alpha(icon.a * 0.16), icon.alpha(icon.a * 0.16 * 0.3));
    let logo = logo_at(k, stem, weak).key("wordmark");
    let boxed = row().key("wordmark-box").w(span).h(span * 42.0 / 234.0).center().child(logo);
    row().key("wordmark-row").w_full().justify_center().child(boxed)
}

/// The logo's blocks at `k` points a unit (234 × 42 units): letters in `stem`, the counters'
/// lower halves in `weak`.
pub(crate) fn logo_at(k: f32, stem: rune::Color, weak: rune::Color) -> El {
    // x, y, w, h in the logo's 234×42 units; `true` for a counter's lower half.
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
