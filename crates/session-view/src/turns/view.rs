//! How a turn's blocks draw (`timeline-next/tools/context-group.tsx`, `assistant-content.tsx`):
//! an activity group, the thoughts a group of thoughts alone is, an events row, a thought, and
//! calls on their own row.

use rune::{El, col, el, row, text};

use super::{Calls, Label, Loc, Step, call, file_tool, grouped_read, label, named_skill};
use crate::TimelineHost;
use crate::activity::{event_row, events, label as activity};
use crate::detail::Category;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, ROW_GAP, UI, arrow, reasoning};

/// Between an open group's label and its first step, and between its steps: 16 from text to
/// text (`--timeline-gap-s`, `--tool-content-gap`), the rows 28 tall around 16 pt lines.
const GROUP_GAP: f32 = ROW_GAP;

/// The folded row (`ContextGroup`): its label, and when open, every step in order, 14 in from a
/// line 2 in from its edge (`timeline-next.css`, nested content). Plain reads in a row share a
/// line, edits, writes and patches in a row share one card of files, named skills in a row share
/// one "Loaded" line, and shells reporting back in a row share one nested events row.
/// The label only records what happened: what is live is the status line's (`StatusLane`), so
/// it never shimmers. A group of thoughts alone is those thoughts, without a group around them;
/// one of shell reports alone is the events row it is. The row springs open and
/// shut on the timeline's growth, the timeline holding as Desktop's does
/// (`disclosure::toggle_group`), and while open the header sticks to the timeline's top until
/// the group scrolls past. `live`: the turn is still running, so a step still under way
/// shimmers.
#[allow(clippy::too_many_arguments)]
pub fn group<S: TimelineHost>(s: &S, p: &Pal, session: &str, key: &str, steps: &[Step], at: &[Loc], turn: std::ops::Range<usize>, live: bool, fade: f32) -> El {
    let open = s.expanded().contains(key);
    if steps.iter().all(|st| matches!(st, Step::Thought { .. })) {
        return col().w_full().gap(ROW_GAP).children(steps.iter().map(|st| thought(s, p, st, live, fade)));
    }
    let work = activity::work_events(steps);
    let work_only = !work.is_empty() && steps.iter().all(is_work);
    if work_only {
        return event_row::view(s, p, session, &work, open, crate::disclosure::toggle_group::<S>(key.to_owned()));
    }
    let shutting = !open && crate::disclosure::is_shutting(s, key);
    let named = activity::activity(steps);
    let words_text = named.as_ref().map_or_else(|| label(steps).text(), activity::Activity::text);
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label(words_text)
        .expanded(open)
        .h(28.0)
        .gap(4.0)
        .center_y()
        .hover_group()
        // The arrow fades in under the pointer (`collapsible.css:26-29`).
        .hover_motion(HOVER_FADE, HOVER_FADE)
        // A step joining or the label changing crossfades the words while the arrow slides over.
        .animate_layout()
        .on_press_tx(crate::disclosure::toggle_group::<S>(key.to_owned()));
    // A card of files in the group holds headers that stick under this one (Desktop's
    // `--sticky-accordion-top`, `message-part.css:1330-1332`).
    let files = steps.iter().any(file_tool);
    if open || shutting {
        trigger = trigger.fill(p.bg_base).sticky_top(crate::style::sticky_top());
        trigger = if files {
            // A solid strip under the label, which a stuck file header meets; the soft edge would
            // wash over that header. It presses and hovers as the label does.
            trigger.child(el().key("pad").pin(Some(0.0), Some(28.0), Some(0.0), None).h(GROUP_GAP).fill(p.bg_base))
        } else {
            // Steps scroll under it and soften into its fill rather than meeting a hard edge, and
            // shutting from there they slide away under it.
            trigger.stuck_shadow(rune::Shadow { y: 10.0, blur: 20.0, color: p.bg_base, ..Default::default() })
        };
    }
    trigger = match named {
        Some(a) => trigger.child(activity_words(p, &a)),
        None => trigger.child(used_words(p, &label(steps))),
    };
    // The arrow shows while the pointer is over the label, and always while open.
    let a = el().key("arrow").w(16.0).h(24.0).ml(2.0).center().child(arrow(p, open));
    trigger = trigger.child(if open || crate::style::hover_none() { a } else { a.group_opacity(0.0, 1.0) });
    let mut c = col().w_full().child(trigger);
    if open || shutting {
        // A file header in the steps sticks under the label and the gap under it.
        let top = crate::style::sticky_top();
        // The steps stack as growth rows on the fold's spring, so a step folding open or shut
        // in the middle slides the ones under it rather than moving them at once (t1484).
        // They're virtual rows: an open group of a thousand calls builds the ones in view, each
        // found again from the transcript by its place (`Loc`), never the whole turn's fold.
        let fold = crate::disclosure::fold(s.knobs());
        let edge = rune::StreamEdge { word_fade: 0.0, ..rune::edge::FEATHER };
        let items = std::rc::Rc::new(plan(steps));
        let pb = (6.0 - items.last().map_or(6.0, |&(i, n)| step_foot(s, session, live, &steps[i..i + n]))).max(0.0);
        let keys: std::rc::Rc<Vec<String>> = std::rc::Rc::new(items.iter().map(|&(i, _)| item_key(steps, i)).collect());
        let at: std::rc::Rc<[Loc]> = at.into();
        let version = (key, &*keys);
        let (count, session, p, group_key) = (items.len(), session.to_owned(), *p, key.to_owned());
        let (row_keys, span) = (keys.clone(), turn);
        let list = col().key("rows").w_full().transition(rune::Transition::none()).growth(rune::GrowthSpec { spring: fold, edge })
            .virtual_rows(count, 36.0, move |_: &S, j| row_keys[j].clone().into(), move |s: &S, j| {
                // Rows build after the group's row returns, maybe beside another timeline's.
                crate::style::set_sticky_top(top);
                let (i, n) = items[j];
                let built = s.with_transcript(&session, |t| {
                    let messages = t.messages.get(span.clone())?;
                    let steps: Vec<Step> = at.get(i..i + n)?.iter().map(|l| l.step(messages)).collect::<Option<_>>()?;
                    let cx = crate::tools::Ctx { session: &session, live, sticky: top + 28.0 + GROUP_GAP };
                    Some(step_item(s, &p, cx, &group_key, &steps, live, fade))
                }).flatten().unwrap_or_else(col);
                // The gap is the row's own, so the list holds rows alone.
                let feather = s.knobs().get(&crate::disclosure::FOLD_FEATHER);
                col().w_full().pt(if j == 0 { 0.0 } else { GROUP_GAP }).child(built).fold_glide(crate::disclosure::fold(s.knobs())).fold_feather(feather)
            })
            .virtual_rows_version(version);
        let line = p.border.alpha(p.border.a * 0.5);
        let rail = el().key("rail").pin(Some(2.0), Some(GROUP_GAP + 6.0), None, Some(6.0)).w(1.0).fill(line);
        let list = col().key("steps").w_full().pt(GROUP_GAP).pb(pb).pl(14.0).transition(rune::Transition::none()).child(rail).child(list);

        // Shutting, the steps stay laid out below a row that's only the header, and the row's
        // clip folds them away as its height springs down to it.
        // The box keeps the steps' path the same either way, so they don't remount on a press.
        let held = col().key("steps-box").w_full().transition(rune::Transition::none());
        c = c.child(if open { held } else { held.h(0.0) }.child(list));
    }
    c
}

/// An activity label's words (`context-tool-group-title[data-activity]`): the verb muted, what it
/// touched in base ink, then a dot and the rest, 6 apart, all at 440.
fn activity_words(p: &Pal, a: &activity::Activity) -> El {
    let mut r = row().key("words").gap(6.0).center_y().shrink().min_w(0.0).child(text(a.verb.clone()).key("verb").font(UI).color(p.muted));
    if !a.main.is_empty() {
        r = r.child(text(a.main.clone()).key("main").font(UI).color(p.text).shrink().min_w(0.0).truncate_fade());
    }
    if !a.others.is_empty() {
        r = r.child(text("\u{00b7}").key("sep").font(UI).color(p.muted))
            .child(text(a.others.join(", ")).key("others").font(UI).color(p.text).shrink().min_w(0.0).truncate_fade());
    }
    r
}

/// "Used 2 Thoughts" or "Updates" (`UsedTitle`).
fn used_words(p: &Pal, l: &Label) -> El {
    let words = |key: &'static str, words: &str| (!words.is_empty()).then(|| text(words.to_owned()).key(key).font(UI).color(p.muted));
    row().key("words").gap(4.0).center_y().shrink().min_w(0.0)
        .children(words("used", &l.before))
        .children(l.count.map(|n| text(n.to_string()).key("count").font(UI.features("'tnum' 1")).color(p.text)))
        .children(words("between", &l.between))
        .children((!l.title.is_empty()).then(|| text(l.title.clone()).key("title").font(UI).color(p.text).truncate_fade()))
        .children(words("after", &l.after))
}

/// A step that is work running on, in an opened group: a background shell reporting back
/// (`ContextGroup`'s `WorkRun`). Subagents never join a group.
fn is_work(st: &Step) -> bool {
    matches!(st, Step::Notice(n) if events::notice_event(n).is_some())
}

/// An open group's rows: each the step at its first index and how many steps it draws (plain
/// reads, file edits, named skills and work running on in a row share one).
fn plan(steps: &[Step]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < steps.len() {
        let run = |f: fn(&Step) -> bool| steps[i..].iter().take_while(|st| f(st)).count();
        let n = match &steps[i] {
            st if is_work(st) => run(is_work),
            st @ Step::Tool { .. } if grouped_read(st) => run(grouped_read),
            st @ Step::Tool { .. } if file_tool(st) => run(file_tool),
            st @ Step::Tool { .. } if named_skill(st) => run(named_skill),
            _ => 1,
        };
        out.push((i, n));
        i += n;
    }
    out
}

/// The key of the group row whose first step is `steps[i]`, as [`step_item`] keys it.
fn item_key(steps: &[Step], i: usize) -> String {
    match &steps[i] {
        st if is_work(st) => format!("work-{i}"),
        Step::Thought { msg, index, .. } => format!("r-{msg}-{index}"),
        Step::Notice(n) => format!("notice-{}", n.id),
        Step::Text { .. } => format!("text-{i}"),
        st @ Step::Tool { id, .. } if grouped_read(st) => format!("reads-{id}"),
        st @ Step::Tool { id, .. } if file_tool(st) => format!("files-{id}"),
        st @ Step::Tool { id, .. } if named_skill(st) => format!("skills-{id}"),
        Step::Tool { id, .. } => format!("step-{id}"),
    }
}

/// Events in a run of work (`ContextGroup`'s `items`): shells reporting back, in order.
fn run_events(steps: &[Step]) -> Vec<events::Event> {
    steps.iter().filter_map(activity::step_event).collect()
}

/// How far inside its box an open group's last row visually ends (`rhythm` subtracts `6` for a
/// closed 28 pt trigger; a row ending at a card or open body has 0 overhang and keeps the 6).
fn step_foot<S: TimelineHost>(s: &S, session: &str, live: bool, steps: &[Step]) -> f32 {
    let Some(first) = steps.first() else { return 6.0 };
    match first {
        st if is_work(st) => 6.0,
        Step::Thought { msg, index, secs, .. } => {
            let open = crate::disclosure::is_open(s, &format!("r-{msg}-{index}"), live && secs.is_none() && s.timeline_detail().expanded(Category::Thinking));
            if open { crate::timeline::rhythm::prose_insets().1 } else { 6.0 }
        }
        Step::Notice(_) => 4.0,
        Step::Text { body } => if body.trim_end().ends_with("```") { 0.0 } else { crate::timeline::rhythm::prose_insets().1 },
        st @ Step::Tool { .. } if grouped_read(st) || named_skill(st) => 6.0,
        st @ Step::Tool { .. } if file_tool(st) => 0.0,
        st @ Step::Tool { files, .. } => {
            if !files.is_empty() {
                return 0.0;
            }
            let cx = crate::tools::Ctx { session, live, sticky: 0.0 };
            if call(st).is_some_and(|c| crate::tools::shows_body(s, cx, &c)) { 0.0 } else { 6.0 }
        }
    }
}

/// One open group's row, `steps` those it draws ([`plan`]).
fn step_item<S: TimelineHost>(s: &S, p: &Pal, cx: crate::tools::Ctx, group: &str, steps: &[Step], live: bool, fade: f32) -> El {
    let Some(first) = steps.first() else { return col() };
    match first {
        st if is_work(st) => {
            let key = format!("{group}:events:{}", item_key(steps, 0));
            let open = s.expanded().contains(&key);
            col().key(format!("work-{key}")).w_full().child(event_row::view(s, p, cx.session, &run_events(steps), open, crate::disclosure::set::<S>(key.clone(), open, String::new())))
        }
        st @ Step::Thought { .. } => thought(s, p, st, live, fade),
        // 28 high like the steps around it (`py-1`).
        Step::Notice(n) => col().key(format!("notice-{}", n.id)).w_full().py(4.0).child(crate::turn_errors::notice(s, p, n, crate::compaction::Run::default())),
        // Commentary is keyed by its place among the group's steps, as it has no id.
        Step::Text { body } => col().w_full().child(crate::timeline::markdown_view::<S>(p, body, fade, false)),
        st @ Step::Tool { .. } if grouped_read(st) => {
            let calls: Vec<crate::tools::Call> = steps.iter().filter_map(call).collect();
            crate::tools::cached(s, p, cx, &calls, || crate::tools::read_group(s, p, cx, &calls)).key(format!("reads-{}", calls[0].id))
        }
        st @ Step::Tool { .. } if file_tool(st) => {
            let calls: Vec<crate::tools::Call> = steps.iter().filter_map(call).collect();
            col().key(format!("files-{}", calls[0].id)).w_full().child(crate::tools::cached(s, p, cx, &calls, || crate::tools::file_group(s, p, cx, &calls)))
        }
        st @ Step::Tool { .. } if named_skill(st) => {
            let calls: Vec<crate::tools::Call> = steps.iter().filter_map(call).collect();
            col().key(format!("skills-{}", calls[0].id)).w_full().child(crate::tools::cached(s, p, cx, &calls, || crate::tools::skill_group(s, p, cx, &calls)))
        }
        st @ Step::Tool { id, files, .. } => {
            let step = col().key(format!("step-{id}")).w_full().child(crate::tools::view(s, p, cx, &call(st).expect("a tool step")));
            step.children(crate::images::gallery(s, p, &format!("step-images-{id}"), files, false, None).map(|g| g.pl(22.0)))
        }
    }
}

/// A thought (`ThoughtRow`): "Thought" and how long it took, opening to the thought. A settled
/// thought starts shut; the one the model is writing (`live`, unfinished) streams in view
/// where Timeline detail expands thinking (`thoughtOpens`). Its key holds it open or shut in a
/// group and on its own row alike.
pub fn thought<S: TimelineHost>(s: &S, p: &Pal, st: &Step, live: bool, fade: f32) -> El {
    let Step::Thought { msg, index, body, secs, .. } = st else { return col() };
    let key = format!("r-{msg}-{index}");
    let streaming = live && secs.is_none();
    let open = crate::disclosure::is_open(s, &key, streaming && s.timeline_detail().expanded(Category::Thinking));
    reasoning::<S>(s, p, &key, body, *secs, live, open, fade).key(key)
}

/// An events row on its own (`TimelineEventRow`), keyed `key`: subagents and background shells
/// launched together, or reporting back one after another.
pub fn events<S: TimelineHost>(s: &S, p: &Pal, session: &str, key: &str, steps: &[Step]) -> El {
    let open = s.expanded().contains(key);
    let list: Vec<events::Event> = steps.iter().filter_map(activity::step_event).collect();
    event_row::view(s, p, session, &list, open, crate::disclosure::toggle_group::<S>(key.to_owned()))
}

/// Calls on their own row (`AssistantContent`, `SessionReadToolGroup`, `FileGroup`), drawn as
/// they draw in a group: one call, plain reads sharing a line, or edits sharing a file card.
pub fn calls<S: TimelineHost>(s: &S, p: &Pal, session: &str, kind: Calls, steps: &[Step], live: bool) -> El {
    let cx = crate::tools::Ctx { session, live, sticky: crate::style::sticky_top() };
    let calls: Vec<crate::tools::Call> = steps.iter().filter_map(call).collect();
    let Some(first) = calls.first() else { return col() };
    match kind {
        Calls::Reads => crate::tools::cached(s, p, cx, &calls, || crate::tools::read_group(s, p, cx, &calls)).key(format!("reads-{}", first.id)),
        Calls::Files => col().key(format!("files-{}", first.id)).w_full().child(crate::tools::cached(s, p, cx, &calls, || crate::tools::file_group(s, p, cx, &calls))),
        Calls::One => {
            let files = match &steps[0] {
                Step::Tool { files, .. } => *files,
                _ => &[],
            };
            col().key(format!("step-{}", first.id)).w_full().child(crate::tools::view(s, p, cx, first))
                .children(crate::images::gallery(s, p, &format!("step-images-{}", first.id), files, false, None).map(|g| g.pl(22.0)))
        }
    }
}

/// A subagent's event on its row (`timeline-subagent`): nothing while its call is still being
/// written.
pub fn subagent<S: TimelineHost>(s: &S, p: &Pal, session: &str, step: &Step) -> El {
    activity::step_event(step).map_or_else(col, |e| event_row::subagent(s, p, session, &e))
}
