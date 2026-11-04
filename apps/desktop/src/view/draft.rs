use super::*;

/// The new-session page, as Desktop's `new-session/view.tsx`: the wordmark about a quarter of
/// the way down, the composer 32 below it with where the session will start under that.
/// `width`: the panel's width last frame.
pub(crate) fn draft_page(s: &App, p: &Pal, width: f32) -> El {
    let content = col()
        .key("draft-content")
        .w_full()
        .max_w(880.0)
        .child(wordmark(p, width))
        .child(col().key("draft-composer").w_full().mt(32.0).gap(4.0).child(composer(s, p).max_w(880.0)).child(location(s, p)));
    col()
        .key("draft")
        .grow(1.0)
        .w_full()
        .child(row().key("header").h(48.0).pl(14.0).center_y().window_drag().child(collapsed_slot(s)))
        // Flex shares that put the wordmark near Desktop's 25.375% from the top at usual sizes.
        .child(el().key("above").grow(0.3))
        .child(col().key("draft-column").w_full().px(24.0).center().child(content))
        .child(el().key("below").grow(0.7))
        .children(s.debug.then(|| status_bar(s, p, None)))
}

/// OpenCode's wordmark (`ui/logo.tsx`) as Desktop's new-session page draws it at rest: every
/// letter in the icon colour at 16%, the counters fainter still. It spans 80% of up to 720 pt,
/// scaled so its 6-unit grid lands on whole points.
pub(in crate::view) fn wordmark(p: &Pal, width: f32) -> El {
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
    let span = 0.8 * if width > 0.0 { (width - 48.0).min(720.0) } else { 720.0 };
    let k = ((span / 39.0).round().max(2.0)) / 6.0;
    let (stem, weak) = (p.icon.alpha(0.16), p.icon.alpha(0.16 * 0.3));
    let mut logo = el().key("wordmark").size(234.0 * k, 42.0 * k);
    for (x, y, w, h, counter) in RECTS {
        logo = logo.child(el().absolute(x * k, y * k).size(w * k, h * k).fill(if counter { weak } else { stem }));
    }
    row().key("wordmark-row").w_full().justify_center().child(logo)
}

/// Where the new session starts: the project's avatar and name, which open the project
/// picker, and its folder, faint at the end.
pub(in crate::view) fn location(s: &App, p: &Pal) -> El {
    let project = s.draft_project();
    let name = project_or_dir(project, &s.draft_dir);
    let dir = s.draft_dir.replace("/Users/demo", "~").replace(&home_dir(), "~");
    let open = s.pickers.open == Some(Kind::Project);
    let button = row()
        .key("project")
        .role("button")
        .label("Choose a project")
        .h(28.0)
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .fill(if open { p.hover } else { Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| { s.open_picker(Kind::Project); if s.pickers.open.is_some() { crate::popover::motion::opening(&s.knobs, tx); } })
        .child(crate::icons::badge(s, p, project, &name))
        .child(text(name.clone()).font(UI).color(if open { p.text } else { p.muted }).truncate_fade());
    row()
        .key("location")
        .w_full()
        .min_h(36.0)
        .pl(8.0)
        .pr(6.0)
        .gap(12.0)
        .center_y()
        .child(button)
        .child(spacer())
        .child(text(dir).font(UI).color(p.faint).truncate_fade().pr(6.0))
}
