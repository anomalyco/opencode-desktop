use super::*;
use crate::compaction::Run;
use crate::fold::Fold;

mod dividers;
mod draft;
mod parts;

pub(crate) use draft::*;
#[cfg(test)]
pub(crate) use dividers::divider_runs;
pub(crate) use parts::*;

// ---------------------------------------------------------------------------------------------
// Session

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
    let pills = if child { Vec::new() } else { pills(s, p, id) };
    let lane = crate::jump::lane(!pills.is_empty());
    let mut page = page.child(crate::jump::over(s, p, id, timeline(s, p, id, t, lane), pills)).child(composer_dock(s, p));
    page = page.children(s.debug.then(|| status_bar(s, p, info)));
    if s.opening.is_some() { page.inert() } else { page }
}

/// The pills over the composer: the subagent capsule, then the todo pill beside it.
fn pills(s: &App, p: &Pal, id: &str) -> Vec<El> {
    let capsule = crate::subagents::capsule(s, p, id);
    let gap = if capsule.is_some() { crate::jump::GAP } else { 0.0 };
    // The timeline's edge rises over their row first, so a pill fades in only once nothing is
    // under it, and goes at once so the edge can come back down (t1129).
    let enter = |pill: El| pill.transition(rune::Transition::asymmetric(rune::Transition::opacity(), rune::Transition::none())).enter_delay(PILL_WAIT);
    capsule.into_iter().chain(crate::todos::pill(s, p, id).map(|t| t.ml(gap))).map(enter).collect()
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
        .children((!pills.is_empty()).then(|| col().key("activity").w_full().center().px(crate::view::DOCK_SIDE).pb(8.0)
            .child(row().key("activity-lane").w_full().max_w(crate::view::DOCK_W).children(pills))))
        .child(composer_dock(s, p))
}

/// One timeline row's content, before it becomes a growth-stack child.
pub(super) struct RowSpec {
    key: String,
    content: RowContent,
    streaming: bool,
}

pub(super) enum RowContent {
    User { message: usize, tight: bool, above: bool, below: bool, fold: Fold },
    /// An undelivered steer: the bubble it becomes once delivered, muted and marked Pending,
    /// under the same row key so delivery keeps it in place.
    Pending { id: String, tight: bool, above: bool, below: bool, fold: Fold },
    Note(usize),
    /// A switched agent or model, a move, updated instructions (`turn_errors`), and what sits
    /// beside it should it draw as a divider.
    Notice(usize, Run),
    /// The turn's last step's error, or its Retry row while the server waits to try it again.
    Error { message: usize, pt: f32 },
    Retry { message: usize, pt: f32 },
    Compaction { message: usize, piece: crate::compaction::Piece, run: Run },
    /// A compaction the server admitted but hasn't started.
    CompactionQueued(Run),
    TurnText { message: usize, part: usize, live: bool, pt: f32 },
    Turn { start: usize, end: usize, block: usize, live: bool, pt: f32 },
    Actions { message: usize, busy: bool },
    /// Empty room this tall: [`CUSHION`] over Working while a reply's words stream in, and the
    /// same at the timeline's end otherwise, so the room moves without the timeline changing height.
    Room(f32),
    /// A prompt's images, and a tool's (by id, in messages `start..end`): built, and so
    /// fetched and decoded, only when the row mounts.
    UserImages { id: String, pt: f32 },
    ToolImages { start: usize, end: usize, tool: String },
    /// Room at the timeline's end for the pills' lane, so its last row stands clear over them
    /// (t1129); a row, so the pinned timeline glides up as it opens rather than jumping.
    Lane(f32),
}

impl RowSpec {
    /// A row that folds open and shut under its header, keyed in `App::expanded` by its row key:
    /// a tool group, or the quiet Compacted divider's summary.
    fn folds(&self) -> bool {
        self.key.starts_with("c-") || matches!(self.content, RowContent::Compaction { piece: crate::compaction::Piece::Quiet, .. })
    }

    /// A streaming row with words in it: a reply's text (not its still-empty start) or a
    /// compaction's summary.
    fn shows_words(&self, t: &Transcript) -> bool {
        let words = |m: usize, part: Option<usize>| match &t.messages[m] {
            Message::Assistant(a) => matches!(part.map_or(a.parts.last(), |i| a.parts.get(i)), Some(Part::Text { text }) if !text.trim().is_empty()),
            _ => false,
        };
        match &self.content {
            RowContent::TurnText { message, part, .. } => words(*message, Some(*part)),
            RowContent::Turn { end, .. } => words(*end - 1, None),
            _ => true,
        }
    }

    fn wrap(&self, s: &App, el: El) -> El {
        let row = col().w_full().max_w(CONTENT_W).streaming(self.streaming).child(el);
        if self.streaming {
            row
        } else if self.folds() {
            // A fold glides on its own quick spring, however long it is. Shutting, what it showed
            // hangs below the header until it unmounts, and Rune clips a row only while its height
            // moves: a short group lands first, and its steps flashed over the rows below (t1107).
            let row = if s.shutting(&self.key) { row.clip_soft(0.0) } else { row };
            row.fold_glide(crate::disclosure::fold(&s.knobs)).fold_feather(s.knobs.get(&crate::disclosure::FOLD_FEATHER))
        } else {
            // Finished content opens in one quick glide, which a pinned timeline follows.
            row.arrives_whole()
        }
    }

    fn build(&self, s: &App, p: &Pal, t: &Transcript, fade: f32) -> El {
        #[cfg(test)]
        let eager = EAGER_ROWS.get();
        #[cfg(not(test))]
        let eager = false;
        let find_ver = s.find.row_version(&self.key);
        if !eager {
            match &self.content {
                RowContent::TurnText { message, part, live, pt } => {
                    if let Message::Assistant(a) = &t.messages[*message]
                        && let Some(Part::Text { text: body }) = a.parts.get(*part)
                    {
                        // Only a reply with a diagram in it follows the column's width.
                        let dw = if body.contains("mermaid") { crate::genui::width(s).to_bits() } else { 0 };
                        let ver = (&t.session, body.as_str(), *live, pt.to_bits(), fade.to_bits(), p.dark, self.streaming, find_ver, dw, s.diagram.hidden_in(body));
                        return rune::memo(self.key.clone(), ver, || {
                            #[cfg(test)]
                            ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                            self.wrap(s, col().w_full().pt(*pt).child(markdown_view_find(s, p, &self.key, body, fade, *live)))
                        });
                    }
                }
                RowContent::Turn { start, end, block, live, pt }
                    if self.key.starts_with("c-") && !s.expanded.contains(&self.key) && !s.shutting(&self.key) =>
                {
                    let fold = crate::disclosure::fold(&s.knobs).duration.to_bits();
                    let ver = (&t.session, group_hash(&t.messages[*start..*end]), *block, *live, pt.to_bits(), p.dark, self.streaming, fold);
                    return rune::memo(self.key.clone(), ver, || {
                        #[cfg(test)]
                        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                        let steps: Vec<&Assistant> = t.messages[*start..*end].iter().filter_map(|m| match m {
                            Message::Assistant(a) => Some(a), _ => None,
                        }).collect();
                        let blocks = turns::fold(&steps, *live);
                        let busy = *live && *block + 1 == blocks.len();
                        let el = match &blocks[*block] {
                            Block::Group { key, steps } => turns::group(s, p, key, steps, busy, fade).pt(*pt),
                            _ => unreachable!(),
                        };
                        self.wrap(s, el)
                    });
                }
                RowContent::Room(h) => {
                    return rune::memo(self.key.clone(), h.to_bits(), || {
                        #[cfg(test)]
                        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                        self.wrap(s, el().h(*h))
                    });
                }
                RowContent::User { message, tight, above, below, fold } => {
                    if let Message::User { id, text, .. } = &t.messages[*message]
                        && !s.glows.contains_key(id)
                        && !s.rewind.menu_open_on(id)
                        && model::user_quotes(id).is_empty()
                        && opencode_kit::mention::user_mentions(id).is_empty()
                        && !text.contains("@ses_")
                    {
                        let open = s.expanded.contains(&crate::fold::key(id));
                        let ver = (&t.session, id.as_str(), text.as_str(), [*tight, *above, *below, fold.past, s.settings.fold_user, open, p.dark, self.streaming], fold.viewport.to_bits(), find_ver);
                        return rune::memo(self.key.clone(), ver, || {
                            #[cfg(test)]
                            ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                            let run = Place { tight: *tight, above: *above, below: *below, fold: *fold };
                            self.wrap(s, user_bubble(s, p, id, text, run, 0.0, None, None))
                        });
                    }
                }
                _ => {}
            }
        }
        #[cfg(test)]
        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
        let el = match &self.content {
            RowContent::User { message, tight, above, below, fold } => match &t.messages[*message] {
                Message::User { id, text, .. } => {
                    let glow = s.glows.get(id).map_or(0.0, |at| crate::knobs::glow(&s.knobs, s.clock - at));
                    let run = Place { tight: *tight, above: *above, below: *below, fold: *fold };
                    user_bubble(s, p, id, text, run, glow, None, crate::quote_cards::on_message(s, p, id, text))
                }
                _ => unreachable!(),
            },
            RowContent::Pending { id, tight, above, below, fold } => {
                let item = s.inbox_for(&t.session).and_then(|i| i.items.iter().find(|i| &i.id == id));
                match item {
                    Some(item) => {
                        let cards = crate::quote_cards::on_message(s, p, id, &item.text);
                        let label = if cards.is_some() { item.text.clone() } else { crate::queue::pending_label(item) };
                        let run = Place { tight: *tight, above: *above, below: *below, fold: *fold };
                        user_bubble(s, p, id, &label, run, 0.0, Some(&t.session), cards)
                    }
                    None => col(),
                }
            }
            RowContent::Note(i) => match &t.messages[*i] {
                Message::Note { text: body, .. } => row().pt(16.0).child(readable(p, text(body.clone()).font(UI).color(p.faint))),
                _ => unreachable!(),
            },
            RowContent::Notice(i, run) => match &t.messages[*i] {
                Message::Notice(n) => crate::turn_errors::notice(s, p, n, *run),
                _ => unreachable!(),
            },
            RowContent::Error { message, pt } => match &t.messages[*message] {
                Message::Assistant(a) => crate::turn_errors::error(s, p, a).pt(*pt),
                _ => unreachable!(),
            },
            RowContent::Retry { message, pt } => match &t.messages[*message] {
                Message::Assistant(Assistant { retry: Some(r), .. }) => crate::turn_errors::retry(s, p, r).pt(*pt),
                _ => col(),
            },
            RowContent::Compaction { message, piece, run } => match &t.messages[*message] {
                Message::Compaction(c) => crate::compaction::view(s, p, c, *piece, fade, *run),
                _ => unreachable!(),
            },
            RowContent::CompactionQueued(run) => crate::compaction::queued(s, p, *run),
            RowContent::Lane(h) => el().w_full().h(*h),
            RowContent::Actions { message, busy } => col().w_full().child(turn_actions(s, p, *busy, &t.session, &t.messages[*message])),
            RowContent::Room(h) => el().h(*h),
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
            RowContent::TurnText { message, part, live, pt } => match &t.messages[*message] {
                Message::Assistant(a) => match &a.parts[*part] {
                    Part::Text { text: body } => col().w_full().pt(*pt).child(markdown_view_find(s, p, &self.key, body, fade, *live)),
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            },
            RowContent::Turn { start, end, block, live, pt } => {
                let steps: Vec<&Assistant> = t.messages[*start..*end].iter().filter_map(|m| match m {
                    Message::Assistant(a) => Some(a), _ => None,
                }).collect();
                let blocks = turns::fold(&steps, *live);
                let busy = *live && *block + 1 == blocks.len();
                match &blocks[*block] {
                    Block::Group { key, steps } => turns::group(s, p, key, steps, busy, fade).pt(*pt),
                    Block::Text { body, live, .. } => col().w_full().pt(*pt).child(markdown_view_find(s, p, &self.key, body, fade, *live)),
                    Block::Agent { .. } => crate::agent_rows_view::row_for(s, p, &t.session, &blocks, *block).pt(*pt),
                    Block::Question { id, status, input, metadata } =>
                        crate::question_row::view(s, p, id, status, input, metadata).pt(*pt),
                }
            }
        };
        self.wrap(s, el)
    }
}

fn group_hash(messages: &[Message]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    for m in messages {
        if let Message::Assistant(a) = m {
            (&a.id, a.completed, a.parts.len()).hash(&mut h);
            for p in &a.parts {
                match p {
                    Part::Tool { id, name, status, .. } => (0u8, id, name, std::mem::discriminant(status)).hash(&mut h),
                    Part::Reasoning { text, completed, .. } => (1u8, text.len(), completed).hash(&mut h),
                    Part::Text { text } => (2u8, text.len()).hash(&mut h),
                }
            }
        }
    }
    h.finish()
}

#[cfg(test)]
thread_local! {
    pub(super) static ROW_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static EAGER_ROWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn timeline_reset_row_builds() {
    ROW_BUILDS.set(0);
}

#[cfg(test)]
pub(crate) fn timeline_row_builds() -> usize {
    ROW_BUILDS.get()
}

/// The timeline's padding under its last row.
const END: f32 = 24.0;

/// The line of space under a streaming reply, over Working: a new line lands there in full while
/// the reply's row grows to fit it, rather than typing in under a clip (p191). It's one line of
/// reply text, more than the most a stream's row trails its text by, and whole pixels so the rows
/// under it stay on the grid. Only while words stream in: otherwise Working sits its lane's
/// usual gap under the last row, and the room waits at the timeline's end (p252).
pub(crate) const CUSHION: f32 = 24.0;

/// The feather, with streaming text drawing past its row's growing edge into [`CUSHION`], its
/// words fading in; other rows still clip as they move.
const STREAM_EDGE: rune::growth::StreamEdge = rune::growth::StreamEdge { overflow: true, ..edge::FEATHER };

/// The timeline, running down to the composer under the pills' `lane` (`jump::over`): at its end
/// its last row stands over the lane, clear of the fade into the composer.
pub(super) fn timeline(s: &App, p: &Pal, id: &str, t: Option<&Transcript>, lane: f32) -> El {
    let e = STREAM_EDGE;
    let mut specs = t.map(|t| rows(s, t)).unwrap_or_default();
    if t.is_some() && lane > 0.0 {
        specs.push(RowSpec { key: "pill-lane".into(), content: RowContent::Lane(lane), streaming: false });
    }
    let specs = std::rc::Rc::new(specs);
    let mut stack = col()
        .key("timeline")
        .w_full()
        .center()
        .px(24.0)
        .pt(8.0)
        .pb(END.max(crate::jump::fade(s, 0.0)))
        // One selection across the whole timeline, as a page's: a drag runs from a prompt
        // through tools and replies, and Cmd+A takes every message on screen.
        .selection_group(true)
        .on_selection_change(crate::quote::selection_changed)
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
        let n = specs.len();
        stack = stack.virtual_rows(n, 80.0,
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
        // Near its top, older messages load above what's on screen.
        .on_scroll_offset(s.pages.watch_top(id))
        // Its edges fade only while rows are under them, and never over a stuck group header.
        .scroll_edge_fade(16.0)
        .child(stack)
}

pub(super) fn rows(s: &App, t: &Transcript) -> Vec<RowSpec> {
    let mut out = Vec::new();
    let busy = t.busy() && !s.stops.has(&t.session);
    // A staged revert hides its boundary message and all after it, and the steers waiting behind them.
    let revert = s.session(&t.session).and_then(|i| i.revert.as_deref());
    let shown = t.visible_len(revert);
    let last_assistant = t.messages[..shown].iter().rposition(|m| matches!(m, Message::Assistant(_)));
    let pending: Vec<&InboxItem> = if revert.is_some() { Vec::new() } else { s.inbox_for(&t.session).map(|i| i.steering().collect()).unwrap_or_default() };
    let fold = crate::fold::place(s, &t.session);
    // Working, or a compaction waiting to start, sits between the last prompt and the steers.
    let queued = if revert.is_none() { s.inboxes.get(&t.session).and_then(|i| i.compaction.as_ref()) } else { None };
    let runs_off = last_assistant.is_some() || queued.is_some();
    // Not the session's start while older messages remain to land above.
    let older = s.pages.more_above(&t.session);
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
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false);
                let above = mi > 0 && joins(&t.messages[mi - 1], m);
                // The last prompt runs on into the pending steers under it, unless Working
                // stands between them.
                let below = match t.messages[..shown].get(mi + 1) {
                    Some(n) => joins(m, n),
                    None => !runs_off && pending.first().is_some_and(|n| prompts_join(body, n)),
                };
                let tight = prompt_images(&mut out, id, older);
                if !body.trim().is_empty() || !model::user_quotes(id).is_empty() {
                    out.push(RowSpec { key: format!("u-{id}"), content: RowContent::User { message: mi, tight, above, below, fold: fold(id) }, streaming: false });
                }
            }
            Message::Note { id, .. } => {
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false);
                out.push(RowSpec { key: format!("n-{id}"), content: RowContent::Note(mi), streaming: false });
            }
            Message::Notice(n) => {
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false);
                out.push(RowSpec { key: format!("n-{}", n.id), content: RowContent::Notice(mi, Run::default()), streaming: false });
            }
            Message::Compaction(c) => {
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false);
                let running = c.status == model::CompactionStatus::Running;
                for piece in crate::compaction::pieces(s, &t.session, c) {
                    let streaming = running && piece == crate::compaction::Piece::Summary;
                    out.push(RowSpec { key: piece.key(&c.id), content: RowContent::Compaction { message: mi, piece, run: Run::default() }, streaming });
                }
            }
        }
    }
    turn_rows(&mut out, &steps, start, shown, busy && revert.is_none());
    let mut over = None;
    if let Some(last) = last_assistant {
        // Working lasts the whole run, like Desktop's session status: each step's message
        // finishes before the next one starts, and those gaps used to blink it out.
        let busy = busy || s.running.contains(&t.session);
        let h = if out.iter().any(|r| r.streaming && r.shows_words(t)) { CUSHION } else { 0.0 };
        out.push(RowSpec { key: "turn-cushion".into(), content: RowContent::Room(h), streaming: false });
        over = Some(h);
        out.push(RowSpec { key: "turn-actions".into(), content: RowContent::Actions { message: last, busy }, streaming: false });
    }
    // A compaction waiting to start sits under Working, above the steers, as Desktop's.
    if let Some(id) = queued {
        out.push(RowSpec { key: format!("cmpq-{id}"), content: RowContent::CompactionQueued(Run::default()), streaming: false });
    }
    // Undelivered steers wait under Working, as Desktop shows them, in the row each becomes
    // when the server delivers it to the end of the transcript; Rune slides it up into place.
    let mut prev = match t.messages[..shown].last() {
        Some(Message::User { text, .. }) if steps.is_empty() && !runs_off => Some(text.as_str()),
        _ => None,
    };
    for (k, item) in pending.iter().enumerate() {
        let above = prev.is_some_and(|x| prompts_join(x, item));
        let below = pending.get(k + 1).is_some_and(|n| prompts_join(&item.text, n));
        let tight = prompt_images(&mut out, &item.id, older);
        out.push(RowSpec { key: format!("u-{}", item.id), content: RowContent::Pending { id: item.id.clone(), tight, above, below, fold: fold(&item.id) }, streaming: false });
        prev = Some(&item.text);
    }
    // The cushion's room when it's shut: the two glide together on one spring, so the
    // timeline's height holds as a stream starts or ends (p252).
    if let Some(h) = over {
        out.push(RowSpec { key: "turn-room".into(), content: RowContent::Room(CUSHION - h), streaming: false });
    }
    dividers::stack(t, &mut out);
    out
}

/// The keys of a transcript's rows, top to bottom.
#[cfg(test)]
pub(crate) fn row_keys(s: &App, t: &Transcript) -> Vec<String> {
    rows(s, t).into_iter().map(|r| r.key).collect()
}

/// A prompt's own images, in a row 8 above its bubble. Returns whether its bubble sits tight
/// under what's above it. With `older` messages still to land above, the first prompt keeps the
/// gap it will have under them.
pub(super) fn prompt_images(out: &mut Vec<RowSpec>, id: &str, older: bool) -> bool {
    let start = out.is_empty() && !older;
    let gallery = crate::attachments::shows(&model::user_files(id));
    let tight = start || gallery;
    if gallery {
        let pt = if start { 8.0 } else { 16.0 };
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
pub(super) fn turn_rows(out: &mut Vec<RowSpec>, steps: &[&Assistant], start: usize, end: usize, live: bool) {
    let blocks = turns::fold(steps, live);
    let n = blocks.len();
    let skip: Vec<bool> = if blocks.iter().any(|b| matches!(b, Block::Agent { .. })) {
        (0..n).map(|i| crate::agent_rows_view::skip(&blocks, i)).collect()
    } else {
        Vec::new()
    };
    // Whether the row above was a tool; `None` above the turn's first row.
    let mut prev: Option<bool> = None;
    for (i, b) in blocks.into_iter().enumerate().filter(|(i, _)| !skip.get(*i).copied().unwrap_or(false)) {
        let tool = !matches!(b, Block::Text { .. });
        let pt = part_gap(prev, tool);
        let content = RowContent::Turn { start, end, block: i, live, pt };
        match b {
            Block::Group { key, steps } => {
                // An open group's steps arrive whole: as a streaming row, opening one paced its
                // height at text-reveal speeds (seconds for a long group) and shutting snapped.
                out.push(RowSpec { streaming: false, content, key });
                // An image a tool returned shows even while its group is closed.
                for st in &steps {
                    if let Step::Tool { id, files, .. } = st
                        && crate::attachments::shows(files)
                    {
                        let content = RowContent::ToolImages { start, end, tool: id.to_string() };
                        out.push(RowSpec { key: format!("ti-{id}"), content, streaming: false });
                    }
                }
            }
            Block::Text { key, msg, part, live, .. } => {
                let content = if end - start == steps.len() { RowContent::TurnText { message: start + msg, part, live, pt } } else { content };
                out.push(RowSpec { content, key, streaming: live });
            }
            Block::Agent { id, .. } => out.push(RowSpec { key: format!("a-{id}"), content, streaming: false }),
            Block::Question { id, .. } => out.push(RowSpec { key: format!("q-{id}"), content, streaming: false }),
        }
        prev = Some(tool);
    }
    // Like Desktop, only the turn's last step reports its error, 24 below the work above it,
    // and only the running one its retry.
    let Some(a) = steps.last() else { return };
    let message = start + steps.len() - 1;
    let pt = if prev.is_some() { 24.0 } else { 0.0 };
    if live && a.retry.is_some() {
        out.push(RowSpec { key: format!("r-{}", a.id), content: RowContent::Retry { message, pt }, streaming: false });
    } else if a.error.is_some() && !a.failure.as_ref().is_some_and(|f| interrupted(&f.kind)) {
        out.push(RowSpec { key: format!("e-{}", a.id), content: RowContent::Error { message, pt }, streaming: false });
    }
}

/// An abort or interruption, which Desktop doesn't report as an error: the turn's tail says
/// Interrupted instead (`view::turn_actions`).
pub(crate) fn interrupted(kind: &str) -> bool {
    let k = kind.to_lowercase();
    k.contains("abort") || k.contains("interrupt")
}
