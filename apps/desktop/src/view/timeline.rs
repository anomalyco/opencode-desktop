use super::*;

// ---------------------------------------------------------------------------------------------
// Session

/// The new-session page, as Desktop's `new-session/view.tsx`: the wordmark about a quarter of
/// the way down, the composer 32 below it with where the session will start under that.
/// `width`: the panel's width last frame.
pub(super) fn draft_page(s: &App, p: &Pal, width: f32) -> El {
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
pub(super) fn wordmark(p: &Pal, width: f32) -> El {
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
    // Mixed in sRGB as the browser composites the 16% group, so it's as faint as Desktop's.
    let (stem, weak) = (srgb_mix(p.bg_base, p.icon, 0.16), srgb_mix(p.bg_base, p.icon, 0.16 * 0.3));
    let mut logo = el().key("wordmark").size(234.0 * k, 42.0 * k);
    for (x, y, w, h, counter) in RECTS {
        logo = logo.child(el().absolute(x * k, y * k).size(w * k, h * k).fill(if counter { weak } else { stem }));
    }
    row().key("wordmark-row").w_full().justify_center().child(logo)
}

/// Where the new session starts: the project's avatar and name, which open the project
/// picker, and its folder, faint at the end.
pub(super) fn location(s: &App, p: &Pal) -> El {
    let project = s.draft_project();
    let name = project.map_or_else(|| s.draft_dir.rsplit('/').next().unwrap_or("").to_owned(), |x| x.name.clone());
    let dir = s.draft_dir.replace(&home_dir(), "~");
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
        .on_press_tx(|s: &mut App, tx: &mut Tx| { s.open_picker(Kind::Project); if s.pickers.open.is_some() { tx.instant(); } })
        .child(crate::icons::badge(s, p, project, &name))
        .child(text(name.clone()).font(UI).color(if open { p.text } else { p.muted }).truncate());
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
        .child(text(dir).font(UI).color(p.faint).truncate().shrink().pr(6.0))
}

pub(crate) fn session_page(s: &App, p: &Pal, id: &str) -> El {
    let t = s.transcripts.get(id).filter(|_| !s.loading_session(id));
    let page = col().key("session").grow(1.0).w_full().child(header(s, p, s.session(id), t));
    session_body(s, p, id, page)
}

/// The session page under its header: the timeline, and what sits under it.
pub(super) fn session_body(s: &App, p: &Pal, id: &str, page: El) -> El {
    let info = s.session(id);
    let t = s.transcripts.get(id).filter(|_| !s.loading_session(id));
    let child = info.is_some_and(|i| i.parent.is_some());
    let mut page = page
        .child(crate::jump::over(s, p, id, timeline(s, p, id, t)))
        .child(requests(s, p, id));
    // A subagent takes no prompts: its status line stands where the composer would.
    page = if child {
        page.child(crate::subagents::bar(s, p, id))
    } else {
        page.children(crate::subagents::capsule(s, p, id).map(|c| col().key("activity").w_full().center().px(24.0).child(c)))
            .child(composer_dock(s, p))
    };
    page = page.children(s.debug.then(|| status_bar(s, p, info)));
    if s.opening.is_some() { page.inert() } else { page }
}

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
    col().key("session")
        .w_full()
        .children(crate::subagents::capsule(s, p, id).map(|c| col().key("activity").w_full().center().px(24.0).child(c)))
        .child(composer_dock(s, p))
}

pub(super) fn header(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>) -> El {
    let mut crumbs = row().key("crumbs").gap(0.0).center_y().shrink();
    if let Some(info) = info {
        let project = s.project(&info.project);
        let proj = project.map_or("", |x| x.name.as_str());
        crumbs = crumbs
            .child(crate::icons::badge(s, p, project, proj).mr(8.0))
            .child(text(proj.to_owned()).font(UI_MED).color(p.faint))
            .child(text("/").font(TextStyle::new(11.0, 500.0)).color(p.faint).pl(8.0).pr(4.0));
        if let Some(parent) = info.parent.as_deref().and_then(|pid| s.session(pid)) {
            let pid = parent.id.clone();
            crumbs = crumbs
                .child(
                    row().key("parent-crumb").role("button").label(format!("Open {}", title_of(parent))).h(24.0).px(4.0).center_y().radius(4.0)
                        .hover(p.hover).hover_motion(INSTANT, INSTANT)
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            s.open(&pid, tx);
                        })
                        .child(text(title_of(parent)).font(UI_MED).color(p.faint).truncate()),
                )
                .child(text("/").font(TextStyle::new(11.0, 500.0)).color(p.faint).pl(4.0).pr(4.0));
        }
        crumbs = crumbs.child(text(title_of(info)).font(UI_MED).color(p.text).truncate().pl(4.0));
        // A running subagent's header spins beside its title (`SessionWorkingIndicator`).
        if info.parent.is_some() && s.subtree_running(&info.id) {
            crumbs = crumbs.child(activity_ring(p).key("working").ml(6.0));
        }
    }
    // Context and spend: the last finished step's context window, the session's cost.
    let last = t.and_then(|t| {
        t.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) if a.tokens.context() > 0 => Some(a.tokens.context()),
            _ => None,
        })
    });
    let ctx = last.unwrap_or(0);
    // The session's own total (it includes subagents); the transcript's sum when it's ahead.
    let summed = t.map_or(0.0, |t| t.messages.iter().map(|m| if let Message::Assistant(a) = m { a.cost } else { 0.0 }).sum::<f64>());
    let cost = info.map_or(0.0, |i| i.cost).max(summed);
    // The session model's context window, or 200k when the server doesn't say.
    let limit = info
        .and_then(|i| s.pickers.limits.get(&format!("{}:{}", i.model.provider, i.model.id)))
        .map_or(200_000.0, |&n| n.max(1) as f32);
    let usage = row()
        .key("usage")
        .gap(6.0)
        .center_y()
        .child(ring(p, (ctx as f32 / limit).min(1.0)))
        .child(text(model::tokens_short(ctx)).font(UI_SMALL).color(p.muted))
        .child(text(format!("${cost:.2}")).font(UI_SMALL).color(p.faint));
    row()
        .key("header")
        .h(48.0)
        .pl(14.0)
        .pr(14.0)
        .gap(8.0)
        .center_y()
        .window_drag()
        .child(collapsed_slot(s))
        .child(crumbs)
        .child(spacer())
        .children(t.filter(|_| last.is_some()).map(|_| usage))
}

/// Context ring: a track circle and an arc, both icons built once per 1/32 step.
pub(super) fn ring(p: &Pal, frac: f32) -> El {
    static ARCS: OnceLock<Vec<Icon>> = OnceLock::new();
    static TRACK: Icon = Icon { name: "ring-track", view: 16.0, body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"# };
    let arcs = ARCS.get_or_init(|| {
        (0..=32)
            .map(|k| {
                let f = k as f32 / 32.0;
                let body = if k == 0 {
                    String::new()
                } else if k == 32 {
                    r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"#.to_owned()
                } else {
                    let a = f * std::f32::consts::TAU;
                    let (x, y) = (8.0 + 6.0 * a.sin(), 8.0 - 6.0 * a.cos());
                    format!(
                        r#"<path d="M8 2A6 6 0 {} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
                        u8::from(f > 0.5)
                    )
                };
                Icon { name: "ring-arc", view: 16.0, body: Box::leak(body.into_boxed_str()) }
            })
            .collect()
    });
    let k = (frac * 32.0).round().clamp(0.0, 32.0) as usize;
    el().key("ring")
        .size(16.0, 16.0)
        .child(icon(&TRACK, 16.0).color(p.layer3).absolute(0.0, 0.0))
        .child(icon(&arcs[k], 16.0).color(p.icon).absolute(0.0, 0.0))
}

/// One timeline row's content, before it becomes a growth-stack child.
pub(super) struct RowSpec {
    key: String,
    content: RowContent,
    streaming: bool,
}

pub(super) enum RowContent {
    User { message: usize, tight: bool, above: bool, below: bool },
    /// An undelivered steer: the bubble it becomes once delivered, muted and marked Pending,
    /// under the same row key so delivery keeps it in place.
    Pending { id: String, tight: bool, above: bool, below: bool },
    Note(usize),
    Turn { start: usize, end: usize, block: usize, live: bool, pt: f32 },
    Actions { message: usize, busy: bool },
    /// A prompt's images, and a tool's (by id, in messages `start..end`): built, and so
    /// fetched and decoded, only when the row mounts.
    UserImages { id: String, pt: f32 },
    ToolImages { start: usize, end: usize, tool: String },
    Built(Box<El>),
}

impl RowSpec {
    fn build(&self, s: &App, p: &Pal, t: &Transcript, fade: f32) -> El {
        #[cfg(test)]
        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
        let el = match &self.content {
            RowContent::User { message, tight, above, below } => match &t.messages[*message] {
                Message::User { id, text, .. } => user_bubble(p, id, text, *tight, *above, *below, s.glows.get(id).map_or(0.0, |at| crate::knobs::glow(&s.knobs, s.clock - at)), None),
                _ => unreachable!(),
            },
            RowContent::Pending { id, tight, above, below } => {
                let item = s.inbox_for(&t.session).and_then(|i| i.items.iter().find(|i| &i.id == id));
                match item {
                    Some(item) => {
                        let label = crate::queue::pending_label(item);
                        user_bubble(p, id, &label, *tight, *above, *below, 0.0, Some(&t.session))
                    }
                    None => col(),
                }
            }
            RowContent::Note(i) => match &t.messages[*i] {
                Message::Note { text: body, .. } => row().pt(16.0).child(text(body.clone()).font(UI).color(p.faint)),
                _ => unreachable!(),
            },
            RowContent::Actions { message, busy } => col().w_full().pt(8.0).child(turn_actions(s, p, *busy, &t.session, &t.messages[*message])),
            RowContent::UserImages { id, pt } => {
                let files = model::user_files(id);
                crate::attachments::gallery(s, p, "images", &files, true).unwrap_or_else(col).pt(*pt)
            }
            RowContent::ToolImages { start, end, tool } => {
                let files = t.messages[*start..*end].iter().find_map(|m| match m {
                    Message::Assistant(a) => a.parts.iter().find_map(|part| match part {
                        Part::Tool { id, files, .. } if id == tool => Some(files),
                        _ => None,
                    }),
                    _ => None,
                });
                files.and_then(|f| crate::attachments::gallery(s, p, "images", f, false)).unwrap_or_else(col).pt(8.0)
            }
            RowContent::Built(el) => (**el).clone(),
            RowContent::Turn { start, end, block, live, pt } => {
                let steps: Vec<&Assistant> = t.messages[*start..*end].iter().filter_map(|m| match m {
                    Message::Assistant(a) => Some(a), _ => None,
                }).collect();
                let blocks = turns::fold(&steps, *live);
                let busy = *live && *block + 1 == blocks.len();
                match &blocks[*block] {
                    Block::Group { key, steps } => turns::group(s, p, key, steps, busy, fade).pt(*pt),
                    Block::Text { body, live, .. } => col().w_full().pt(*pt).child(markdown_view(p, body, fade, *live)),
                    Block::Agent { .. } => crate::agent_rows_view::row_for(s, p, &t.session, &blocks, *block).pt(*pt),
                    Block::Question { id, status, input, metadata } =>
                        crate::question_row::view(s, p, id, status, input, metadata).pt(*pt),
                }
            }
        };
        let row = col().w_full().max_w(CONTENT_W).streaming(self.streaming).child(el);
        // Finished content opens in one quick glide, which a pinned timeline follows.
        if self.streaming { row } else { row.arrives_whole() }
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static ROW_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static EAGER_ROWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn timeline(s: &App, p: &Pal, id: &str, t: Option<&Transcript>) -> El {
    let e = edge::ALL[s.edge];
    let specs = std::rc::Rc::new(t.map(|t| rows(s, p, t)).unwrap_or_default());
    let mut stack = col()
        .key("timeline")
        .w_full()
        .center()
        .px(24.0)
        .pt(8.0)
        .pb(24.0)
        .growth(GrowthSpec { spring: crate::knobs::growth(&s.knobs), edge: e });
    #[cfg(test)]
    let eager = EAGER_ROWS.get();
    #[cfg(not(test))]
    let eager = false;
    // Eager construction is the benchmark and regression-test reference.
    if eager && let Some(t) = t {
        stack = stack.virtualize(80.0)
            .children(specs.iter().map(|r| r.build(s, p, t, e.word_fade).key(r.key.clone())));
    } else if t.is_some() {
        let keys = specs.clone();
        let id = id.to_owned();
        let p = *p;
        stack = stack.virtual_rows(specs.len(), 80.0,
            move |_: &App, i| keys[i].key.clone().into(),
            move |s: &App, i| specs[i].build(s, &p, &s.transcripts[&id], e.word_fade));
    }
    if t.is_none() {
        stack = stack.child(
            row()
                .key("loading")
                .w_full()
                .max_w(CONTENT_W)
                .h(40.0)
                .gap(8.0)
                .center()
                .child(activity_ring(p))
                .child(text(s.opening.as_ref().filter(|o| o.id == id).and_then(|o| o.error.as_deref()).unwrap_or("Loading messages…")).font(UI).color(p.faint)),
        );
    }
    col()
        .key(format!("scroll-{id}"))
        .grow(1.0)
        .shrink()
        .w_full()
        .scroll(ScrollSpec { stick_bottom: true })
        .clip_fade(0.0, 16.0, 0.0, 8.0)
        .child(stack)
}

pub(super) fn rows(s: &App, p: &Pal, t: &Transcript) -> Vec<RowSpec> {
    let mut out = Vec::new();
    let busy = t.busy();
    // A staged revert hides its boundary message and all after it, and the steers waiting behind them.
    let revert = s.session(&t.session).and_then(|i| i.revert.as_deref());
    let shown = t.visible_len(revert);
    let last_assistant = t.messages[..shown].iter().rposition(|m| matches!(m, Message::Assistant(_)));
    let pending: Vec<&InboxItem> = if revert.is_some() { Vec::new() } else { s.inbox_for(&t.session).map(|i| i.steering().collect()).unwrap_or_default() };
    // A turn's consecutive steps fold together (see `turns`); prompts and notes end them.
    let mut steps: Vec<&Assistant> = Vec::new();
    let mut start = 0;
    for (mi, m) in t.messages[..shown].iter().enumerate() {
        match m {
            Message::Assistant(a) => {
                if steps.is_empty() { start = mi; }
                steps.push(a);
            }
            Message::User { id, text: body, .. } => {
                turn_rows(s, p, &mut out, &std::mem::take(&mut steps), start, mi, false);
                let above = mi > 0 && joins(&t.messages[mi - 1], m);
                // The last prompt runs on into the pending steers under it.
                let below = match t.messages[..shown].get(mi + 1) {
                    Some(n) => joins(m, n),
                    None => pending.first().is_some_and(|n| prompts_join(body, n)),
                };
                let tight = prompt_images(&mut out, id);
                if !body.trim().is_empty() {
                    out.push(RowSpec { key: format!("u-{id}"), content: RowContent::User { message: mi, tight, above, below }, streaming: false });
                }
            }
            Message::Note { id, .. } => {
                turn_rows(s, p, &mut out, &std::mem::take(&mut steps), start, mi, false);
                out.push(RowSpec { key: format!("n-{id}"), content: RowContent::Note(mi), streaming: false });
            }
        }
    }
    turn_rows(s, p, &mut out, &steps, start, shown, busy && revert.is_none());
    // Undelivered steers wait below the running work and above Working, as Desktop shows them,
    // in the row each becomes when the server delivers it to the end of the transcript.
    let mut prev = match t.messages[..shown].last() {
        Some(Message::User { text, .. }) if steps.is_empty() => Some(text.as_str()),
        _ => None,
    };
    for (k, item) in pending.iter().enumerate() {
        let above = prev.is_some_and(|x| prompts_join(x, item));
        let below = pending.get(k + 1).is_some_and(|n| prompts_join(&item.text, n));
        let tight = prompt_images(&mut out, &item.id);
        out.push(RowSpec { key: format!("u-{}", item.id), content: RowContent::Pending { id: item.id.clone(), tight, above, below }, streaming: false });
        prev = Some(&item.text);
    }
    if let Some(last) = last_assistant {
        // Working lasts the whole run, like Desktop's session status: each step's message
        // finishes before the next one starts, and those gaps used to blink it out.
        let busy = busy || s.running.contains(&t.session);
        out.push(RowSpec { key: "turn-actions".into(), content: RowContent::Actions { message: last, busy }, streaming: false });
    }
    out
}

/// A prompt's own images, in a row 8 above its bubble. Returns whether its bubble sits tight
/// under what's above it.
pub(super) fn prompt_images(out: &mut Vec<RowSpec>, id: &str) -> bool {
    let gallery = crate::attachments::shows(&model::user_files(id));
    let tight = out.is_empty() || gallery;
    if gallery {
        let pt = if out.is_empty() { 8.0 } else { 16.0 };
        out.push(RowSpec { key: format!("ui-{id}"), content: RowContent::UserImages { id: id.to_owned(), pt }, streaming: false });
    }
    tight
}

/// Space above a turn's row, as Desktop spaces assistant parts: none under the prompt, 8 between
/// two tools, 16 before anything else.
pub(super) fn part_gap(prev: Option<bool>, tool: bool) -> f32 {
    match prev {
        None => 0.0,
        Some(true) if tool => 8.0,
        Some(_) => 16.0,
    }
}

/// `live`: these steps end the transcript and the last one is still running.
pub(super) fn turn_rows(s: &App, p: &Pal, out: &mut Vec<RowSpec>, steps: &[&Assistant], start: usize, end: usize, live: bool) {
    let blocks = turns::fold(steps, live);
    let n = blocks.len();
    let skip: Vec<bool> = (0..n).map(|i| crate::agent_rows_view::skip(&blocks, i)).collect();
    // Whether the row above was a tool; `None` above the turn's first row.
    let mut prev: Option<bool> = None;
    for (i, b) in blocks.into_iter().enumerate().filter(|(i, _)| !skip[*i]) {
        let tool = !matches!(b, Block::Text { .. });
        let pt = part_gap(prev, tool);
        let content = RowContent::Turn { start, end, block: i, live, pt };
        out.push(match b {
            Block::Group { key, steps } => {
                let busy = live && i + 1 == n;
                out.push(RowSpec { streaming: busy && s.expanded.contains(&key), content, key });
                // An image a tool returned shows even while its group is closed.
                for st in &steps {
                    if let Step::Tool { id, files, .. } = st
                        && crate::attachments::shows(files)
                    {
                        let content = RowContent::ToolImages { start, end, tool: id.to_string() };
                        out.push(RowSpec { key: format!("ti-{id}"), content, streaming: false });
                    }
                }
                prev = Some(true);
                continue;
            }
            Block::Text { key, live, .. } => RowSpec { content, key, streaming: live },
            Block::Agent { id, .. } => RowSpec {
                key: format!("a-{id}"),
                content,
                streaming: false,
            },
            Block::Question { id, .. } => RowSpec { key: format!("q-{id}"), content, streaming: false },
        });
        prev = Some(tool);
    }
    // Like Desktop, only the turn's last step reports its error, 24 below the work above it.
    if let Some(a) = steps.last()
        && let Some(err) = &a.error
    {
        let pt = if prev.is_some() { 24.0 } else { 0.0 };
        out.push(RowSpec { key: format!("e-{}", a.id), content: RowContent::Built(Box::new(row().pt(pt).child(text(err.clone()).font(UI).color(p.danger).wrap().w_full()))), streaming: false });
    }
}

/// 64ch of the bubble's 14 px Inter: Desktop caps a bubble at `min(82%, 64ch)`.
pub(super) const BUBBLE_MAX: f32 = 565.0;

/// Whether prompt `b` joins the run of prompt `a` just above it: both have words and `b` brings
/// no files (Desktop's `user-runs.ts`).
pub(super) fn joins(a: &Message, b: &Message) -> bool {
    match (a, b) {
        (Message::User { text: x, .. }, Message::User { id, text: y }) => {
            !x.trim().is_empty() && !y.trim().is_empty() && model::user_files(id).is_empty()
        }
        _ => false,
    }
}

/// Whether pending steer `b` joins the run of the prompt (or steer) with text `a` above it.
pub(super) fn prompts_join(a: &str, b: &InboxItem) -> bool {
    !a.trim().is_empty() && !b.text.trim().is_empty() && b.files.is_empty() && model::user_files(&b.id).is_empty()
}

/// A prompt's bubble. A run of prompts sits 4 apart with the trailing corners tightened where
/// they meet (`above`, `below`); the run's last prompt has a copy line under it, and the others
/// show copy beside the bubble, all only while hovered.
///
/// `pending` (the session) draws an undelivered steer: muted, with Move to queue and Delete in
/// copy's place, and the run's last one marked Pending under its bubble at all times.
pub(super) fn user_bubble(p: &Pal, id: &str, body: &str, tight: bool, above: bool, below: bool, glow: f32, pending: Option<&str>) -> El {
    let words = body.to_owned();
    let copy = match pending {
        Some(session) => crate::queue::pending_actions(p, session, id, below),
        None => row()
            .key("copy")
            .role("button")
            .label("Copy message")
            .size(20.0, 20.0)
            .center()
            .radius(4.0)
            .hover(p.hover)
            .press(p.pressed)
            .group_opacity(0.0, 1.0)
            .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.copy(words.clone()))
            .child(icon(&ic::OUTLINE_COPY, 14.0).color(p.icon_muted)),
    };
    let (top, end) = (if above { 4.0 } else { 10.0 }, if below { 4.0 } else { 10.0 });
    let mut bubble = col()
        .key("bubble")
        .max_w(BUBBLE_MAX)
        .shrink()
        .px(12.0)
        .py(8.0)
        .corners([10.0, top, end, 10.0])
        .fill(if pending.is_some() { p.layer1 } else { p.layer2 });
    if glow > 0.0 {
        // A soft inner glow and a slight lift, under the text.
        let (rim, lift) = if p.dark { (p.accent.mix(Color::WHITE, 0.45), p.text) } else { (p.accent, Color::WHITE) };
        bubble = bubble.child(
            el().key("glow").absolute(0.0, 0.0).w_full().h_full().corners([10.0, top, end, 10.0])
                .fill(lift.alpha(0.1))
                .box_shadow(Shadow { blur: 14.0, color: rim.alpha(0.55), inset: true, ..Shadow::default() })
                .opacity(glow)
                .opacity_motion(INSTANT),
        );
    }
    let ink = if pending.is_some() { p.muted } else { p.text };
    bubble = bubble.child(text(body.to_owned()).key(format!("text-{id}")).font(BUBBLE).color(ink).wrap());
    let copy = if below {
        let x = if pending.is_some() { -52.0 } else { -28.0 };
        bubble = bubble.child(copy.absolute(x, 8.5));
        None
    } else {
        Some(copy)
    };
    let c = col()
        .w_full()
        .pt(if above { 4.0 } else if tight { 8.0 } else { 16.0 })
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .child(row().w_full().justify_end().child(row().w_frac(0.82).justify_end().child(bubble)));
    c.children(copy.map(|copy| row().key("actions").w_full().h(20.0).mt(4.0).justify_end().child(copy)))
}

pub(crate) fn chevron(open: bool) -> &'static Icon {
    if open { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }
}

pub(crate) fn reasoning(p: &Pal, key: &str, body: &str, secs: Option<i64>, open: bool, fade: f32) -> El {
    let k = key.to_owned();
    let label = match secs {
        Some(s) => format!("Thought for {s}s"),
        None => "Thinking".into(),
    };
    let mut c = col().w_full().child(
        row()
            .key("trigger")
            .role("button")
            .h(28.0)
            .gap(6.0)
            .center_y()
            // "Thinking" crossfades to "Thought for Ns" while the chevron slides over.
            .animate_layout()
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                // Opening and closing is instant; only the streaming label change animates.
                tx.instant();
                if !s.expanded.remove(&k) {
                    s.expanded.insert(k.clone());
                }
            })
            .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted))
            .child(text(label).key(if secs.is_some() { "thought" } else { "thinking" }).font(UI).color(p.muted))
            .child(icon(chevron(open), 14.0).color(p.icon_faint)),
    );
    if open {
        c = c.child(
            row().key("thought").pl(22.0).pt(4.0).child(text(body.to_owned()).font(UI.lh(20.0)).color(p.muted).wrap().reveal(fade).w_full()),
        );
    }
    c
}

pub(super) fn turn_actions(s: &App, p: &Pal, busy: bool, session: &str, message: &Message) -> El {
    // These share a footprint and swap at once: a crossfade would lay the copy icon's outline
    // over the W in Working.
    let has_text = matches!(message, Message::Assistant(a) if a.parts.iter().any(|part| matches!(part, Part::Text { text } if !text.trim().is_empty())));
    let mut button = row().key("copy").role("button").label("Copy response")
        .size(28.0, 28.0).pb(12.0).center().radius(4.0).hover(p.hover).press(p.pressed)
        .opacity(if busy || !has_text { 0.0 } else { 1.0 })
        .opacity_motion(INSTANT)
        .child(icon(&ic::OUTLINE_COPY, 16.0).color(p.muted));
    if !busy && has_text {
        let session = session.to_owned();
        let id = message.id().to_owned();
        button = button.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if let Some(Message::Assistant(a)) = s.transcripts.get(&session).and_then(|t| t.messages.iter().find(|m| m.id() == id)) {
                tx.copy(a.parts.iter().filter_map(|part| match part {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n\n"));
            }
        });
    }
    // One reserved tail: completion crossfades in place rather than first removing Working's
    // height and then adding actions. The scroll endpoint cannot reverse during this handoff.
    el().w_full().h(28.0).motion(crate::knobs::actions_tail(&s.knobs))
        .child(crate::working::row(s, p, session, busy).key("working").absolute(0.0, 0.0).opacity(if busy { 1.0 } else { 0.0 }).opacity_motion(INSTANT))
        .child(button.absolute(0.0, 0.0))
}

// ---------------------------------------------------------------------------------------------
// Markdown

pub(super) fn markdown_view(p: &Pal, src: &str, fade: f32, streaming: bool) -> El {
    let kit = Kit::neutral(p.dark);
    let style = MarkdownStyle::from_kit(&kit)
        .text(p.text)
        .muted(p.muted)
        .link(p.accent)
        .inline_code_bg(p.inline_code_bg)
        .code_block(p.layer1, p.border_muted)
        .quote_rule(p.layer3)
        .rule(p.border)
        .body_font(MD)
        .heading_font(1, H1)
        .heading_font(2, H2)
        .heading_font(3, H3)
        .code_block_font(CODE);
    Markdown::new(src).style(style).reveal(fade).streaming(streaming).on_link(|s: &mut App, url: &str| s.open_link(url)).build(&kit)
}

// ---------------------------------------------------------------------------------------------
// Requests: permission asks and questions waiting on you, docked above the composer.

pub(super) fn small_button(p: &Pal, key: String, label: &str, primary: bool) -> El {
    row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .h(28.0)
        .px(10.0)
        .center_y()
        .radius(6.0)
        .fill(if primary { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI_MED).color(p.text))
}

pub(super) fn requests(s: &App, p: &Pal, id: &str) -> El {
    let mut dock = col().key("requests").w_full().center().px(24.0).gap(8.0);
    // Questions take the composer's place (`question.rs`).
    for r in s.pending_for(id).into_iter().filter(|r| matches!(r, Pending::Permission { .. })) {
        dock = dock.child(request_card(p, r));
    }
    dock
}

pub(super) fn request_card(p: &Pal, r: &Pending) -> El {
    let rid = r.id().to_owned();
    let card = col()
        .key(format!("request-{rid}"))
        .w_full()
        .max_w(CONTENT_W)
        .padding(14.0)
        .gap(10.0)
        .radius(12.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .border(0.5, p.accent);
    match r {
        Pending::Permission { action, resources, message, .. } => {
            let mut c = card.child(text(format!("Allow {action}?")).font(UI_MED).color(p.text));
            if !message.is_empty() {
                c = c.child(text(message.clone()).font(UI).color(p.muted).wrap());
            }
            for (i, res) in resources.iter().take(6).enumerate() {
                c = c.child(text(res.clone()).key(format!("res-{i}")).font(CODE).color(p.muted).truncate());
            }
            let b = |key: &str, label: &str, decision: &'static str, primary: bool| {
                let rid = rid.clone();
                small_button(p, format!("{key}-{rid}"), label, primary).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.reply_permission(&rid, decision);
                })
            };
            c.child(
                row()
                    .key("actions")
                    .gap(6.0)
                    .child(b("once", "Allow once", "once", true))
                    .child(b("always", "Always allow", "always", false))
                    .child(b("reject", "Reject", "reject", false)),
            )
        }
        Pending::Form { .. } => card,
    }
}
