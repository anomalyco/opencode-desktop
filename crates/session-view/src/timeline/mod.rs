//! Shared virtualized session timeline: message rows, assistant turn folding, streamed line
//! reveal, disclosure glides, scroll follower, and edge fades (`p255`).

use opencode_kit::model::{self, Assistant, InboxItem, Message, Part, Transcript};
use rune::{El, GrowthSpec, ScrollSpec, col, edge, el, row, text};

use crate::TimelineHost;
use crate::compaction::Run;
use crate::fold::Fold;
use crate::palette::Pal;
use crate::style::{UI, activity_ring};
use crate::turns::{self, Block, Entry, Step};

mod dividers;
#[cfg(test)]
mod open_tests;
pub mod parts;

pub use dividers::divider_runs;
pub use parts::*;

/// One timeline row's content, before it becomes a growth-stack child.
pub struct RowSpec {
    pub key: String,
    pub content: RowContent,
    pub streaming: bool,
}

pub enum RowContent {
    User { message: usize, tight: bool, above: bool, below: bool, fold: Fold },
    /// An undelivered steer: the bubble it becomes once delivered, muted and marked Pending,
    /// under the same row key so delivery keeps it in place.
    Pending { id: String, tight: bool, above: bool, below: bool, fold: Fold },
    /// A switched agent or model, a move, updated instructions (`turn_errors`), and what sits
    /// beside it should it draw as a divider.
    Notice(usize, Run),
    /// The turn's last step's error, or its Retry row while the server waits to try it again.
    Error { message: usize, pt: f32 },
    Retry { message: usize, pt: f32 },
    Compaction { message: usize, piece: crate::compaction::Piece, run: Run },
    /// A compaction the server admitted but hasn't started.
    CompactionQueued(Run),
    /// Between a stopped step and the steps the run went on with: Interrupted.
    TurnDivider(Run),
    TurnText { message: usize, part: usize, live: bool, pt: f32 },
    Turn { start: usize, end: usize, block: usize, live: bool, pt: f32 },
    /// `asking`: the session waits on a question or permission. `last`: it ends the timeline and
    /// nothing waits to send, so a finished reply's footer takes room of its own (`parts::turn_actions`).
    Actions { message: usize, busy: bool, asking: bool, last: bool },
    /// A prompt's images, and a tool's (by id, in messages `start..end`): built, and so
    /// fetched and decoded, only when the row mounts.
    UserImages { id: String, pt: f32 },
    ToolImages { start: usize, end: usize, tool: String },
    /// Changed files: the run that ended at message `idle` (its idle marker), from prompt
    /// `from`, which changed `files`.
    /// `pt`: the room over Changed files (`pt-4`, less what the reply's footer leaves under it).
    TurnDiff { idle: usize, from: String, files: Vec<String>, pt: f32 },
    /// Room at the timeline's end for the pills' lane, so its last row stands clear over them
    /// (t1129); a row, so the pinned timeline glides up as it opens rather than jumping.
    Lane(f32),
}

impl RowSpec {
    /// A row that folds open and shut under its header, keyed in `expanded` by its row key:
    /// a tool group, or the quiet Compacted divider's summary.
    fn folds(&self) -> bool {
        self.key.starts_with("c-") || matches!(self.content, RowContent::Compaction { piece: crate::compaction::Piece::Quiet, .. })
    }

    fn wrap<S: TimelineHost>(&self, s: &S, el: El) -> El {
        self.wrap_paced(s, el, false)
    }

    /// [`Self::wrap`], with `paced` streaming rows climbing at the stream's pace rather than a
    /// line at a time: a reply with a fenced block in it, whose diagram, outline or code can
    /// grow by many lines in one build.
    fn wrap_paced<S: TimelineHost>(&self, s: &S, el: El, paced: bool) -> El {
        let row = col().w_full().max_w(crate::style::content_w()).child(el);
        let k = s.knobs();
        if self.streaming && (paced || crate::line_reveal::reveal(k) == crate::line_reveal::Reveal::HeldRoom) {
            row.streaming(true)
        } else if self.streaming {
            // Streaming words open their row a line at a time, each line on its own quick spring
            // the moment it wraps, clipped at its moving edge: the line shows whole almost at once,
            // nothing paints over Working, and Working steps down by just that line, however
            // slowly the words come (p252, `line_reveal`).
            row.fold_glide(crate::line_reveal::line_grow(k)).fold_feather(crate::line_reveal::feather(k))
        } else if self.folds() {
            // A fold glides on its own quick spring, however long it is. Shutting, what it showed
            // hangs below the header until it unmounts, and Rune clips a row only while its height
            // moves: a short group lands first, and its steps flashed over the rows below (t1107).
            let row = if crate::disclosure::is_shutting(s, &self.key) { row.clip_soft(0.0) } else { row };
            row.fold_glide(crate::disclosure::fold(k)).fold_feather(k.get(&crate::disclosure::FOLD_FEATHER))
        } else {
            // Finished content opens in one quick glide, which a pinned timeline follows.
            row.arrives_whole()
        }
    }

    /// A reply's text part, its words held back for Grow, then show (`line_reveal`). The text
    /// the footer copies (`footer`) shares the footer's hover.
    #[allow(clippy::too_many_arguments)]
    fn text_row<S: TimelineHost>(&self, s: &S, p: &Pal, message: &str, part: usize, body: &str, fade: f32, live: bool, pt: f32, footer: bool) -> El {
        let md = |text: &str, fade: f32| markdown_view_find(s, p, &self.key, text, fade, live, Some(message));
        let words = crate::line_reveal::words(col().w_full().pt(pt), md, body, fade, s.line_hold().shown(message, part));
        let row = self.wrap_paced(s, words, body.contains("```"));
        if footer { row.hover_group_shared(REPLY_HOVER).hover_motion(crate::style::INSTANT, crate::style::INSTANT) } else { row }
    }

    pub fn build<S: TimelineHost>(&self, s: &S, p: &Pal, t: &Transcript, fade: f32) -> El {
        let eager = EAGER_ROWS.get();
        let find_ver = s.find_row_version(&self.key);
        let k = s.knobs();
        if !eager {
            match &self.content {
                RowContent::TurnText { message, part, live, pt } => {
                    if let Message::Assistant(a) = &t.messages[*message]
                        && let Some(Part::Text { text: body }) = a.parts.get(*part)
                    {
                        // Only a reply with a diagram in it follows the column's width.
                        let dw = if body.contains("mermaid") { crate::genui::width(s).to_bits() } else { 0 };
                        let reveal = crate::line_reveal::reveal(k);
                        let line = (reveal, k.get(&crate::line_reveal::LINE_GROW).to_bits(), crate::line_reveal::feather(k).to_bits(), s.line_hold().shown(&a.id, *part));
                        // A code block's Copy shows its tick, so the copy in hand rebuilds the row.
                        let copied = body.contains("```").then(|| crate::copied::current(s)).flatten();
                        // A picture landing (an image, a link's favicon) or the viewer opening one rebuilds the row.
                        let pictures = (body.contains("![") || body.contains("://")).then(|| {
                            let m = s.media_store();
                            (m.markdown.version(), m.viewing.as_ref().map(|v| v.id.clone()))
                        });
                        let footer = footer_text(t, *message, *part);
                        // A session id it names being looked up, renamed, starting or stopping.
                        let chips = body.contains("ses_").then(|| crate::session_chips::version(s, body));
                        let ver = (&t.session, body.as_str(), *live, pt.to_bits(), fade.to_bits(), p.dark, self.streaming, find_ver, dw, s.diagrams().hidden_in(body), line, (copied, pictures, footer, crate::style::content_w().to_bits(), chips));
                        return rune::memo(self.key.clone(), ver, || {
                            ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                            self.text_row(s, p, &a.id, *part, body, fade, *live, *pt, footer)
                        });
                    }
                }
                RowContent::Turn { start, end, block, live, pt }
                    if self.key.starts_with("c-") && !s.expanded().contains(&self.key) && !crate::disclosure::is_shutting(s, &self.key) =>
                {
                    let fold = crate::disclosure::fold(k).duration.to_bits();
                    // A live Thinking group's time ticks: the row rebuilds once a second, and not
                    // at all once the turn ends.
                    let tick = if *live { thinking_tick(s, t, *start, *end, *block) } else { None };
                    let detail = s.timeline_detail();
                    let ver = (&t.session, group_hash(&t.messages[*start..*end]), *block, *live, pt.to_bits(), p.dark, self.streaming, (fold, tick, crate::style::content_w().to_bits(), detail));
                    return rune::memo(self.key.clone(), ver, || {
                        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                        let blocks = turns::fold_with(&entries(&t.messages[*start..*end], &detail), *live, &detail);
                        let busy = *live && *block + 1 == blocks.len();
                        let el = match blocks.get(*block) {
                            Some(Block::Group { key, steps }) => turns::group(s, p, &t.session, key, steps, busy, *live, fade).pt(*pt),
                            _ => col(),
                        };
                        self.wrap(s, el)
                    });
                }
                RowContent::User { message, tight, above, below, fold } => {
                    if let Message::User { id, text, .. } = &t.messages[*message]
                        && s.user_glow_at(id).is_none()
                        && !s.user_menu_open_on(id)
                        && model::user_quotes(id).is_empty()
                        && opencode_kit::mention::user_mentions(id).is_empty()
                        && !text.contains("ses_")
                    {
                        let open = s.expanded().contains(&crate::fold::key(id));
                        let meta = prompt_meta(s, t, Some(*message));
                        let ver = (&t.session, id.as_str(), text.as_str(), [*tight, *above, *below, fold.past, s.fold_user_messages(), open, p.dark, self.streaming], fold.viewport.to_bits(), find_ver, s.user_status_version(id), &meta, crate::style::content_w().to_bits());
                        return rune::memo(self.key.clone(), ver, || {
                            ROW_BUILDS.set(ROW_BUILDS.get() + 1);
                            let run = Place { tight: *tight, above: *above, below: *below, fold: *fold };
                            self.wrap(s, user_bubble(s, p, &t.session, id, text, run, 0.0, false, None, &meta))
                        });
                    }
                }
                _ => {}
            }
        }
        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
        let el = match &self.content {
            RowContent::User { message, tight, above, below, fold } => match &t.messages[*message] {
                Message::User { id, text, .. } => {
                    let glow = s.user_glow_at(id).map_or(0.0, |at| crate::knobs::glow(k, s.clock() - at));
                    let run = Place { tight: *tight, above: *above, below: *below, fold: *fold };
                    user_bubble(s, p, &t.session, id, text, run, glow, false, crate::user_parts::quotes_on_message(s, p, id, text), &prompt_meta(s, t, Some(*message)))
                }
                _ => unreachable!(),
            },
            RowContent::Pending { id, tight, above, below, fold } => {
                let item = s.inbox_for(&t.session).and_then(|i| i.items.iter().find(|i| &i.id == id));
                match item {
                    Some(item) => {
                        let cards = crate::user_parts::quotes_on_message(s, p, id, &item.text);
                        let label = if cards.is_some() { item.text.clone() } else { s.pending_steer_label(item) };
                        let run = Place { tight: *tight, above: *above, below: *below, fold: *fold };
                        user_bubble(s, p, &t.session, id, &label, run, 0.0, true, cards, &prompt_meta(s, t, None))
                    }
                    None => col(),
                }
            }
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
                Message::Compaction(c) => crate::compaction::view(s, p, c, *piece, *run),
                _ => unreachable!(),
            },
            RowContent::CompactionQueued(run) => crate::compaction::queued(s, p, *run),
            RowContent::TurnDivider(run) => crate::compaction::divider_of(s, p, text(opencode_i18n::t!("ui.message.interrupted")).key("label").font(UI).color(p.faint), *run),
            RowContent::Lane(h) => el().w_full().h(*h),
            RowContent::TurnDiff { idle, from, files, pt } => crate::turn_diff::view(s, p, &t.session, t.messages[*idle].id(), from, files, *pt),
            // Held room keeps a line of room over Working, which streaming text draws into (#304).
            RowContent::Actions { message, busy, asking, last } => col().w_full().pt(crate::line_reveal::room(s)).child(turn_actions(s, p, *busy, *asking, *last, &t.session, &t.messages[*message])),
            RowContent::UserImages { id, pt } => {
                let files = model::user_files(id);
                crate::images::gallery(s, p, "images", &files, true, Some(id)).unwrap_or_else(col).pt(*pt)
            }
            RowContent::ToolImages { start, end, tool } => {
                let files = t.messages[*start..*end].iter().find_map(|m| match m {
                    Message::Assistant(a) => a.parts.iter().find_map(|part| match part {
                        Part::Tool { id, files, .. } if id == tool => Some(files),
                        _ => None,
                    }),
                    _ => None,
                });
                files.and_then(|f| crate::images::gallery(s, p, "images", f, false, None)).unwrap_or_else(col).pt(8.0)
            }
            RowContent::TurnText { message, part, live, pt } => match &t.messages[*message] {
                Message::Assistant(a) => match &a.parts[*part] {
                    Part::Text { text: body } => return self.text_row(s, p, &a.id, *part, body, fade, *live, *pt, footer_text(t, *message, *part)),
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            },
            RowContent::Turn { start, end, block, live, pt } => {
                let detail = s.timeline_detail();
                let blocks = turns::fold_with(&entries(&t.messages[*start..*end], &detail), *live, &detail);
                let busy = *live && *block + 1 == blocks.len();
                match blocks.get(*block) {
                    Some(Block::Group { key, steps }) => turns::group(s, p, &t.session, key, steps, busy, *live, fade).pt(*pt),
                    Some(Block::Text { key, body, live, .. }) => {
                        // `t-{message}-{n}`
                        let message = key.strip_prefix("t-").and_then(|k| k.rsplit_once('-')).map(|(id, _)| id);
                        col().w_full().pt(*pt).child(markdown_view_find(s, p, &self.key, body, fade, *live, message))
                    }
                    Some(Block::Agent { .. }) => crate::subagent_card::row_for(s, p, &t.session, &blocks, *block).pt(*pt),
                    Some(Block::Question { id, status, input, metadata }) =>
                        crate::question_row::view(s, p, &t.session, id, status, input, metadata).pt(*pt),
                    Some(Block::Thought { step, .. }) => turns::thought(s, p, step, *live, fade).pt(*pt),
                    Some(Block::Calls { kind, steps, .. }) => turns::calls(s, p, &t.session, *kind, steps, *live).pt(*pt),
                    None => col(),
                }
            }
        };
        self.wrap(s, el)
    }
}

/// The seconds a live turn's group `block` has been thinking, when it reads Thinking.
fn thinking_tick<S: TimelineHost>(s: &S, t: &Transcript, start: usize, end: usize, block: usize) -> Option<i64> {
    let detail = s.timeline_detail();
    let blocks = turns::fold_with(&entries(&t.messages[start..end], &detail), true, &detail);
    match blocks.get(block)? {
        Block::Group { steps, .. } => turns::thinking_since(steps, block + 1 == blocks.len()).map(|at| turns::thinking_secs(at, s.now_ms())),
        _ => None,
    }
}

/// A turn's messages as its entries, one for each: its steps, the notices that fold with its
/// work, and what it passes over.
pub fn entries<'a>(messages: &'a [Message], detail: &crate::detail::Detail) -> Vec<Entry<'a>> {
    messages.iter().map(|m| match m {
        Message::Assistant(a) => Entry::Step(a),
        Message::Note { step, .. } => Entry::Step(step),
        Message::Notice(n) if crate::detail::notice_placement(n, detail) == crate::detail::Placement::Grouped => Entry::Notice(n),
        _ => Entry::Skip,
    }).collect()
}

fn group_hash(messages: &[Message]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    for m in messages {
        if let Message::Notice(n) = m {
            (3u8, &n.id).hash(&mut h);
        }
        let step = match m {
            Message::Assistant(a) => Some(a),
            Message::Note { step, .. } => Some(&**step),
            _ => None,
        };
        if let Some(a) = step {
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

thread_local! {
    pub static ROW_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub static EAGER_ROWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn timeline_reset_row_builds() {
    ROW_BUILDS.set(0);
}

pub fn timeline_row_builds() -> usize {
    ROW_BUILDS.get()
}

/// Desktop's `tailSize`, the room under the timeline's last row (`virtualizer.tsx:152`, its
/// `paddingEnd`): the Working lane at its top, then the gap over the composer. The turn's
/// actions row (`parts::turn_actions`) holds that lane, so the padding is the rest, and stays
/// put: steers waiting under the actions row end the timeline with the gap alone.
const TAIL: f32 = 92.0;

/// The timeline, running down to the composer under the pills' `lane` (`jump::over`): at its end
/// its last row stands over the lane, clear of the fade into the composer.
pub fn timeline<S: TimelineHost>(s: &S, p: &Pal, id: &str, t: Option<&Transcript>, lane: f32) -> El {
    // Held room's streaming text draws past its row's edge into the room over Working.
    let held_room = crate::line_reveal::reveal(s.knobs()) == crate::line_reveal::Reveal::HeldRoom;
    let e = rune::growth::StreamEdge { overflow: held_room, ..edge::FEATHER };
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
        .pb((TAIL - crate::working::LANE).max(crate::jump::fade(s, 0.0)))
        // One selection across the whole timeline, as a page's: a drag runs from a prompt
        // through tools and replies, and Cmd+A takes every message on screen.
        .selection_group(true)
        .on_selection_change(S::on_timeline_selection_change)
        // While a panel's spring moves the timeline's edge, rows take the width they land at,
        // so their text wraps once, and glide from where the first in view was
        // (`virtualizer.tsx:254-268`, `panel-motion.ts` `holdLandingWidth`).
        .hold_landing_width()
        .growth(GrowthSpec { spring: crate::knobs::growth(s.knobs()), edge: e });
    let eager = EAGER_ROWS.get();
    // Eager construction is the benchmark and regression-test reference.
    if eager && let Some(t) = t {
        stack = stack.virtualize(80.0)
            .children(specs.iter().map(|r| r.build(s, p, t, e.word_fade).key(r.key.clone())));
    } else if t.is_some() {
        let keys = specs.clone();
        let id = id.to_owned();
        let p = *p;
        let n = specs.len();
        stack = stack.virtual_rows(
            n,
            80.0,
            move |_: &S, i| keys[i].key.clone().into(),
            move |s: &S, i| s.with_transcript(&id, |t| specs[i].build(s, &p, t, e.word_fade)).unwrap_or_else(col),
        );
    }
    if t.is_none() {
        stack = stack.child(
            row()
                .key("loading")
                .w_full()
                .max_w(crate::style::content_w())
                .h(40.0)
                .gap(8.0)
                .center()
                .child(activity_ring(p))
                .child(text(s.loading_error(id).unwrap_or(opencode_i18n::t!("session.messages.loading"))).font(UI).color(p.faint)),
        );
    }
    let mut scroll = col()
        .key(format!("scroll-{id}"))
        .grow(1.0)
        .shrink()
        .w_full()
        .scroll(ScrollSpec { stick_bottom: true })
        // Its edges fade only while rows are under them, and never over a stuck group header.
        .scroll_edge_fade(16.0)
        .child(stack);
    // Near its top, older messages load above what's on screen.
    if let Some(top) = s.watch_scroll_top(id) {
        scroll = scroll.on_scroll_offset(top);
    }
    scroll
}

pub fn rows<S: TimelineHost>(s: &S, t: &Transcript) -> Vec<RowSpec> {
    let mut out = Vec::new();
    let busy = s.timeline_busy(t) && !s.is_session_stopped(&t.session);
    // A staged revert hides its boundary message and all after it, and the steers waiting behind them.
    let revert = s.session_revert(&t.session);
    let shown = t.visible_len(revert);
    let last_assistant = t.messages[..shown].iter().rposition(|m| matches!(m, Message::Assistant(_)));
    let pending: Vec<&InboxItem> = if revert.is_some() { Vec::new() } else { s.inbox_for(&t.session).map(|i| i.steering().collect()).unwrap_or_default() };
    let fold = crate::fold::place(s, &t.session);
    // Working, or a compaction waiting to start, sits between the last prompt and the steers.
    let queued = if revert.is_none() { s.queued_compaction(&t.session) } else { None };
    // A new session's first prompt has no reply above it yet, and Working shows under it all the same.
    let actions = last_assistant.or_else(|| (busy && revert.is_none() && shown > 0).then(|| shown - 1));
    let runs_off = actions.is_some() || queued.is_some();
    // Not the session's start while older messages remain to land above.
    let older = s.more_above(&t.session);
    // A turn's consecutive steps fold together (see `turns`), with the notices Timeline detail
    // groups; prompts, notes and notices on their own rows end them, and hidden notices don't.
    let detail = s.timeline_detail();
    let mut steps: Vec<Entry> = Vec::new();
    let mut start = 0;
    // `steps` began with a shell command, whose turn holds only it and its notices.
    let mut shell_turn = false;
    // Each shell command's turn: where its first row goes, its message and the room over it.
    let mut shell_gaps: Vec<(usize, usize, f32)> = Vec::new();
    // The run under way (`execution`): its first prompt and the files its steps changed, and
    // where the last run's Changed files went.
    let mut run: Option<(String, std::collections::BTreeSet<String>)> = None;
    let mut diff_at: Option<usize> = None;
    for (mi, m) in t.messages[..shown].iter().enumerate() {
        match m {
            Message::Assistant(a) => {
                // A reply doesn't join a shell command's turn.
                if std::mem::take(&mut shell_turn) {
                    turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false, &detail);
                }
                if let Some(r) = &mut run {
                    r.1.extend(a.changed.iter().cloned());
                }
                if steps.is_empty() { start = mi; }
                steps.push(Entry::Step(a));
                // Steps after an interrupted one (the run went on after a stop) sit under an
                // Interrupted divider (`projection.ts:554-582`, `session-timeline-row.tsx:1162-1182`),
                // which hidden notices hide too.
                if a.failure.as_ref().is_some_and(|f| interrupted(&f.kind)) && matches!(t.messages[..shown].get(mi + 1), Some(Message::Assistant(_))) {
                    turn_rows(&mut out, &std::mem::take(&mut steps), start, mi + 1, false, &detail);
                    if detail.placement(crate::detail::Category::Notices) != crate::detail::Placement::Hidden {
                        out.push(RowSpec { key: format!("d-{}", a.id), content: RowContent::TurnDivider(Run::default()), streaming: false });
                    }
                }
            }
            // A run ends: with Changed files on, the files it changed list after its work.
            Message::Notice(n) if n.kind == model::NoticeKind::Idle => match run.take().filter(|r| s.turn_diffs() && !r.1.is_empty()) {
                Some((from, files)) => {
                    turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false, &detail);
                    diff_at = Some(out.len());
                    out.push(RowSpec { key: format!("td-{}", n.id), content: RowContent::TurnDiff { idle: mi, from, files: files.into_iter().collect(), pt: 16.0 }, streaming: false });
                }
                None if !steps.is_empty() => steps.push(Entry::Skip),
                None => {}
            },
            Message::Notice(n) if !steps.is_empty() || crate::detail::notice_placement(n, &detail) == crate::detail::Placement::Grouped => {
                match crate::detail::notice_placement(n, &detail) {
                    crate::detail::Placement::Grouped => {
                        if steps.is_empty() { start = mi; }
                        steps.push(Entry::Notice(n));
                    }
                    crate::detail::Placement::Hidden => steps.push(Entry::Skip),
                    crate::detail::Placement::Separate => {
                        turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false, &detail);
                        out.push(RowSpec { key: format!("n-{}", n.id), content: RowContent::Notice(mi, Run::default()), streaming: false });
                    }
                }
            }
            Message::Notice(n) => {
                if crate::detail::notice_placement(n, &detail) == crate::detail::Placement::Separate {
                    out.push(RowSpec { key: format!("n-{}", n.id), content: RowContent::Notice(mi, Run::default()), streaming: false });
                }
            }
            Message::User { id, text: body, .. } => {
                run.get_or_insert_with(|| (id.clone(), Default::default()));
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false, &detail);
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
            // A shell command run from the composer is a turn of its own: its shell call, on its
            // own row or in a group as Timeline detail places shells, with the notices that
            // group after it (`projection.ts` Shell rows, `groupMessages`). It shows unless
            // Shell is hidden, and a failed one always.
            Message::Note { step, .. } => {
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false, &detail);
                // Desktop's TurnGap over every turn but the first (16), whose row sits 8 lower
                // than a turn's rows under a prompt.
                shell_gaps.push((out.len(), mi, if out.is_empty() { 8.0 } else { 16.0 }));
                start = mi;
                steps.push(Entry::Step(step));
                shell_turn = true;
            }
            Message::Compaction(c) => {
                turn_rows(&mut out, &std::mem::take(&mut steps), start, mi, false, &detail);
                for piece in crate::compaction::pieces(c) {
                    out.push(RowSpec { key: piece.key(&c.id), content: RowContent::Compaction { message: mi, piece, run: Run::default() }, streaming: false });
                }
            }
        }
    }
    turn_rows(&mut out, &steps, start, shown, busy && revert.is_none(), &detail);
    for (at, mi, gap) in shell_gaps {
        if let Some(RowSpec { content: RowContent::Turn { start, pt, .. }, .. }) = out.get_mut(at)
            && *start == mi
        {
            *pt = gap;
        }
    }
    if let Some(last) = actions {
        // Working lasts the whole run, like Desktop's session status: each step's message
        // finishes before the next one starts, and those gaps used to blink it out.
        let busy = busy || s.is_session_live(&t.session);
        let actions = RowSpec { key: "turn-actions".into(), content: RowContent::Actions { message: last, busy, asking: busy && s.session_asking(&t.session), last: false }, streaming: false };
        // The reply's footer stays with its words, above the run's Changed files.
        match diff_at.filter(|at| at + 1 == out.len()) {
            Some(at) => {
                out.insert(at, actions);
                // Desktop's footer ends at its Copy (`text-part-copy-wrapper`, 8 over a 20 high
                // row); the lane reserved for Working runs on 8 under it, which Changed files takes
                // from its own 16.
                if let Some(RowSpec { content: RowContent::TurnDiff { pt, .. }, .. }) = out.get_mut(at + 1) {
                    *pt -= crate::working::LANE - crate::working::COPY_AT.1 - crate::working::COPY;
                }
            }
            None => out.push(actions),
        }
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
    dividers::stack(t, &mut out);
    // Input waiting to send (queued, or steers) lands as the turn ends, so the reply's footer
    // takes its room only once nothing waits.
    let waiting = s.inbox_for(&t.session).is_some_and(|i| !i.items.is_empty() || i.compaction.is_some());
    if let Some(RowSpec { content: RowContent::Actions { last, .. }, .. }) = out.last_mut() {
        *last = !waiting;
    }
    out
}

/// The keys of a transcript's rows, top to bottom.
pub fn row_keys<S: TimelineHost>(s: &S, t: &Transcript) -> Vec<String> {
    rows(s, t).into_iter().map(|r| r.key).collect()
}

/// A prompt's own images, in a row 8 above its bubble. Returns whether its bubble sits tight
/// under what's above it. With `older` messages still to land above, the first prompt keeps the
/// gap it will have under them.
pub fn prompt_images(out: &mut Vec<RowSpec>, id: &str, older: bool) -> bool {
    let start = out.is_empty() && !older;
    let gallery = crate::images::shows(&model::user_files(id));
    let tight = start || gallery;
    if gallery {
        let pt = if start { 8.0 } else { 16.0 };
        out.push(RowSpec { key: format!("ui-{id}"), content: RowContent::UserImages { id: id.to_owned(), pt }, streaming: false });
    }
    tight
}

/// Space above a turn's row, as Desktop spaces assistant parts (`session-timeline-row.tsx:696-700`):
/// none under the prompt but a text part's own 24 (`message-part.css:569-571`), 8 between two
/// tools, 16 before anything else.
pub fn part_gap(prev: Option<bool>, tool: bool) -> f32 {
    match prev {
        None if !tool => 24.0,
        None => 0.0,
        Some(true) if tool => 8.0,
        Some(_) => 16.0,
    }
}

/// A turn's rows, `steps` being its messages `start..end` one for one. `live`: these steps end
/// the transcript and the last one is still running.
pub fn turn_rows(out: &mut Vec<RowSpec>, steps: &[Entry], start: usize, end: usize, live: bool, detail: &crate::detail::Detail) {
    let blocks = turns::fold_with(steps, live, detail);
    // Whether the row above was a tool; `None` above the turn's first row.
    let mut prev: Option<bool> = None;
    for (i, b) in blocks.into_iter().enumerate() {
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
                        && crate::images::shows(files)
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
            Block::Thought { key, .. } | Block::Calls { key, .. } => out.push(RowSpec { key, content, streaming: false }),
        }
        prev = Some(tool);
    }
    // Like Desktop, only the turn's last step reports its error, 24 below the work above it,
    // and only the running one its retry.
    let Some((at, a)) = steps.iter().enumerate().rev().find_map(|(i, e)| match e {
        Entry::Step(a) => Some((i, *a)),
        _ => None,
    }) else { return };
    let message = start + at;
    let pt = if prev.is_some() { 24.0 } else { 0.0 };
    if live && a.retry.is_some() {
        out.push(RowSpec { key: format!("r-{}", a.id), content: RowContent::Retry { message, pt }, streaming: false });
    } else if a.error.is_some() && !a.failure.as_ref().is_some_and(|f| interrupted(&f.kind)) {
        // The error's row always sits 24 below what's above it, the prompt too (`pt-6`).
        out.push(RowSpec { key: format!("e-{}", a.id), content: RowContent::Error { message, pt: 24.0 }, streaming: false });
    }
}

/// An abort or interruption, which Desktop doesn't report as an error: the turn's tail says
/// Interrupted instead (`parts::turn_actions`).
pub fn interrupted(kind: &str) -> bool {
    let k = kind.to_lowercase();
    k.contains("abort") || k.contains("interrupt")
}
