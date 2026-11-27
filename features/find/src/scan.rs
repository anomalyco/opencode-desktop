//! Incremental transcript row copying, background index/match scheduling, and result landing
//! for session timeline Find.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use opencode_kit::model::{Message, Part};
use rune::Tx;
use rune_kit::search::Fold;

use crate::matches::seek_active;
use crate::transcript_index::{Index, KEY_BUDGET, Kind, Results, Source, retire};
use crate::{Effect, Event, FindHost, State};

/// What the matches are: none; waiting for the transcript's index to land; those of the rows on
/// screen while the worker matches the rest (`reveal`: the first match in the transcript lights
/// and comes into view when they land); or every row's, for the transcript of shape `shape`.
#[derive(Default)]
pub(crate) enum Answer {
    #[default]
    None,
    Waiting { query: String, reveal: bool },
    Partial { query: String, results: Arc<Results>, reveal: bool },
    Complete { query: String, shape: u64, results: Arc<Results> },
}

/// What a copy of the transcript is checked against: its session, version (with whether it
/// streams and whether outlines read as words) and how many messages show.
pub(crate) type CopyKey = (String, u64, usize);

/// Rows copied a frame (`find_shape`): ~4 ms at 100,000 messages.
pub(crate) const COPY_BUDGET: usize = 16_384;

/// Find's copy of the open transcript's rows: none under way; under way, `done` messages read
/// and hashed as it goes; or done.
#[derive(Default)]
pub(crate) enum Copying {
    #[default]
    Idle,
    Running { next: Source, done: usize, hash: DefaultHasher },
    Done(Source),
}

/// Find's index worker: idle, or making the index of transcript shape `shape`. It has its own
/// worker, so keys arriving faster than a large index builds never restart the build.
#[derive(Default, PartialEq)]
pub(crate) enum Indexing {
    #[default]
    Idle,
    Running { shape: u64 },
}

/// Where a new query's lit match goes: the first in the session (official's `queryChanged`), or,
/// for Find opened on a search result, the first in that message once every match is known.
#[derive(Clone, Default, PartialEq)]
pub(crate) enum Target {
    #[default]
    First,
    Message(String),
}

/// Find's match worker: idle, or matching `query` in the index of shape `shape`.
#[derive(Default, PartialEq)]
pub(crate) enum Matching {
    #[default]
    Idle,
    Running { shape: u64, query: String },
}

fn str_sig(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h = DefaultHasher::new();
    b.len().hash(&mut h);
    b[..b.len().min(32)].hash(&mut h);
    b[b.len().saturating_sub(32)..].hash(&mut h);
    h.finish()
}

fn search_fold(s: &dyn FindHost) -> Fold {
    Fold::for_language(s.search_fold_language())
}

/// Find's target is a message the open session hasn't loaded, and older pages remain.
pub(crate) fn target_waits(s: &dyn FindHost) -> bool {
    let Some(open) = s.find().open() else { return false };
    let (Target::Message(message), Some(id)) = (&open.target, s.active_session_id()) else {
        return false;
    };
    let loaded = s.transcript(id).is_some_and(|t| t.messages.iter().any(|m| m.id() == message));
    !loaded && s.has_unloaded_older_messages(id)
}

/// Keeps Find's session up with the route: another session's timeline keeps Find open on
/// that session's matches (`session.changed`), and leaving the session view closes it, as
/// Desktop's search goes with its session screen (`disposed`).
fn sync_session(s: &mut dyn FindHost) -> Option<String> {
    let Some(id) = s.active_session_id().map(str::to_owned) else {
        let dirty = {
            let f = s.find();
            f.is_open() || !f.input.text.is_empty() || f.input.focused
        };
        if dirty {
            let f = s.find_mut();
            for effect in f.state.step(Event::Left) {
                match effect {
                    Effect::Release(open) => f.finish_close(open),
                    Effect::RetireAnswer(answer) => retire(answer),
                    Effect::FocusInput | Effect::BlurInput | Effect::Reveal => {}
                }
            }
            f.input.clear();
            f.input.focused = false;
        }
        return None;
    };
    if s.find().open().is_some_and(|o| o.session != id) {
        let f = s.find_mut();
        f.matcher.cancel();
        for effect in f.state.step(Event::SessionChanged { session: &id }) {
            match effect {
                Effect::RetireAnswer(answer) => retire(answer),
                Effect::Release(open) => f.finish_close(open),
                Effect::FocusInput | Effect::BlurInput | Effect::Reveal => {}
            }
        }
    }
    Some(id)
}

/// Brings Find's index and matches up to the open session and the query. A key matches at
/// most [`KEY_BUDGET`] rows itself: past that, it lights the rows on screen and the worker
/// matches the rest (and makes the index when the transcript changed).
pub fn sync_index(s: &mut dyn FindHost) {
    let _ = sync_index_inner(s);
}

/// [`sync_index`], returning whether the caller should scroll the active match into view now.
pub(crate) fn sync_index_inner(s: &mut dyn FindHost) -> bool {
    let current = sync_session(s);
    // Settings covers the session, which keeps its Find as it was.
    if s.settings_open() {
        return false;
    }
    let current = current.unwrap_or_default();
    if !s.find().is_open() || current.is_empty() {
        if let Some(open) = s.find_mut().open_mut()
            && !matches!(open.answer, Answer::None)
        {
            retire(std::mem::take(&mut open.answer));
        }
        return false;
    }
    let Some(shape) = find_shape(s, &current) else {
        if let Some(open) = s.find_mut().open_mut()
            && !matches!(open.answer, Answer::None)
        {
            retire(std::mem::take(&mut open.answer));
        }
        return false;
    };
    let q = s.find().query().to_owned();
    let indexed = s.find().open().is_some_and(|o| o.index.as_ref().is_some_and(|i| i.source.shape == shape));
    let mut should_seek = false;
    let waiting = s.find().open().is_some_and(|o| matches!(o.answer, Answer::Waiting { .. }));
    if q.is_empty() {
        if let Some(open) = s.find_mut().open_mut() {
            open.reveal_on_land = false;
            if !matches!(open.answer, Answer::None) {
                retire(std::mem::take(&mut open.answer));
            }
        }
    } else if s.find().answered() != Some(q.as_str()) || (indexed && waiting) {
        let fold = search_fold(s);
        let (current_index, want_reveal) = s
            .find()
            .open()
            .map(|o| (o.index.clone(), o.reveal_on_land))
            .unwrap_or((None, false));
        let answer = match current_index {
            Some(index) if indexed => match index.candidates_within(&q, KEY_BUDGET) {
                // Few rows can match: all of them, now (including right when `f.indexer` lands
                // after a tab switch or early keystroke, avoiding a second worker hop).
                Some(rows) => Answer::Complete { query: q.clone(), shape, results: Arc::new(index.search(&rows, &q, fold)) },
                // Too many: the first match in the session, which official lights and brings into
                // view on every new query (`search-machine.ts` `queryChanged`, `seek(.., 0)`), found
                // by scanning from the start, and the rows on screen; the worker counts the rest.
                None => {
                    let mut rows = rows_on_screen(s, &current, &index);
                    let first = index.first_match(&q, fold, KEY_BUDGET);
                    rows.extend(first);
                    rows.sort_unstable();
                    rows.dedup();
                    Answer::Partial {
                        query: q.clone(),
                        results: Arc::new(index.search(&rows, &q, fold)),
                        reveal: want_reveal && first.is_none(),
                    }
                }
            },
            // The index is a change behind (or not made yet): wait for `f.indexer`, keeping
            // `want_reveal` false across session switches (`search-machine.ts:154-160`).
            _ => Answer::Waiting { query: q.clone(), reveal: want_reveal },
        };
        let complete = {
            let Some(open) = s.find_mut().open_mut() else { return false };
            retire(std::mem::replace(&mut open.answer, answer));
            matches!(open.answer, Answer::Complete { .. })
        };
        let waits = complete && target_waits(s);
        let f = s.find_mut();
        let Some(open) = f.open_mut() else { return false };
        let had_target = open.target != Target::First;
        // A new query lights its first match (`queryChanged`), and only a match that is the
        // first in the session is lit before the rest are counted.
        // Find opened on a search result's message lights that message's first match once
        // every match is known.
        let first = match (&open.answer, &open.target) {
            (Answer::Waiting { .. }, _) | (Answer::Partial { .. }, Target::Message(_)) | (Answer::Partial { reveal: true, .. }, _) => None,
            (Answer::Complete { results, .. }, Target::Message(id)) => results.first_in_message(id).or(Some(0)),
            _ => Some(0),
        };
        // The message isn't loaded yet: older pages are read (`poll`) and the target waits.
        if complete && !waits {
            open.target = Target::First;
            open.reveal_on_land = false;
            should_seek = (want_reveal || had_target) && first.is_some();
        } else if matches!(open.answer, Answer::Partial { reveal: false, .. }) && !had_target {
            open.reveal_on_land = false;
            should_seek = want_reveal && first.is_some();
        }
        drop(f.state.step(Event::MatchesSynced { kept: first }));
        should_seek = should_seek && f.count() > 0;
    }
    // The index for this shape, once (keys never restart it).
    if !indexed && s.find().open().is_some_and(|o| o.indexing != (Indexing::Running { shape })) {
        let f = s.find_mut();
        if let State::Open(open) = &mut f.state {
            open.indexing = Indexing::Running { shape };
            let source = open.source.clone().unwrap_or_default();
            let prev = open.index.clone();
            f.indexer.request((source, prev), |(source, prev): (Arc<Source>, Option<Arc<Index>>)| {
                Arc::new(Index::build(source, prev.as_deref()))
            });
        }
    }
    // Every match of the query, when a key couldn't match it all (or matched it on the last
    // shape), once the index for this shape is in.
    let job = Matching::Running { shape, query: q.clone() };
    let should_match = {
        let Some(open) = s.find().open() else { return should_seek };
        let stale = matches!(&open.answer, Answer::Complete { shape: ans_shape, .. } if *ans_shape != shape);
        let wanted = !q.is_empty() && (matches!(open.answer, Answer::Partial { .. }) || stale);
        indexed && wanted && open.matching != job && open.index.is_some()
    };
    if should_match {
        let fold = search_fold(s);
        let f = s.find_mut();
        if let State::Open(open) = &mut f.state
            && let Some(index) = open.index.clone()
        {
            open.matching = job;
            f.matcher.request((index, q), move |(index, q): (Arc<Index>, String)| {
                let rows = index.candidates(&q);
                let results = Arc::new(index.search(&rows, &q, fold));
                (index.source.shape, q, results)
            });
        }
    }
    should_seek
}

/// The open transcript's shape, once its rows are copied into [`crate::OpenFind::source`]. A change is
/// copied a slice a frame ([`COPY_BUDGET`] rows that changed), so opening Find on 100,000
/// messages spreads its copy over a few frames; the shape before stays meanwhile.
fn find_shape(s: &mut dyn FindHost, current: &str) -> Option<u64> {
    let (key, live, outlines) = {
        let t = s.transcript(current)?;
        let revert = s.session_revert(current);
        let shown = t.visible_len(revert);
        // Outlines read as their words only while a plugin draws them (Rune UI).
        let outlines = s.code_block_lang("ui");
        let live = t.busy() && !s.is_session_stopped(current) && revert.is_none();
        let version = s.transcript_version(current);
        let key = (current.to_owned(), version << 2 | u64::from(live) << 1 | u64::from(outlines), shown);
        (key, live, outlines)
    };
    if let Some((k, shape)) = s.find().open().and_then(|o| o.checked.as_ref())
        && *k == key
    {
        return Some(*shape);
    }
    let copying = std::mem::take(&mut s.find_mut().open_mut()?.copying);
    let copying = copy_rows(s, current, &key, copying, live, outlines);
    let open = s.find_mut().open_mut()?;
    let before = open.checked.as_ref().map(|c| c.1).filter(|_| open.source.as_ref().is_some_and(|src| src.shape != 0));
    match copying {
        Copying::Done(source) => {
            let shape = source.shape;
            if open.source.as_ref().is_none_or(|src| src.shape != shape) {
                retire(open.source.replace(Arc::new(source)));
            } else {
                retire(source);
            }
            open.checked = Some((key, shape));
            Some(shape)
        }
        running => {
            open.copying = running;
            before
        }
    }
}

/// Copies the session's searchable rows (`u-{id}` and `t-{a.id}-{n}`, in timeline order; a
/// reply still streaming keeps its last part even while empty), [`COPY_BUDGET`] rows a call.
/// A copy under way goes on over the transcript as it is now, so one that changes every frame
/// still lands; the next change starts another.
fn copy_rows(s: &dyn FindHost, current: &str, key: &CopyKey, copying: Copying, live: bool, outlines: bool) -> Copying {
    let Some(t) = s.transcript(current) else {
        return Copying::Idle;
    };
    let shown = key.2.min(t.messages.len());
    let (mut next, mut done, mut hash) = match copying {
        Copying::Running { next, done, hash } => (next, done, hash),
        _ => {
            let mut hash = DefaultHasher::new();
            (live, outlines).hash(&mut hash);
            (Source::new(0), 0, hash)
        }
    };
    let last_reply = live
        .then(|| t.messages[..shown].iter().rposition(|m| matches!(m, Message::Assistant(_))))
        .flatten()
        .filter(|&i| t.messages[i + 1..shown].iter().all(|m| matches!(m, Message::Assistant(_))));
    let push = |next: &mut Source, hash: &mut DefaultHasher, id: &str, n: u32, kind: Kind, body: &str| {
        let sig = str_sig(body);
        (id, n, kind as u8, sig).hash(hash);
        next.push(id, n, kind, body, sig);
    };
    let budget = next.len() + COPY_BUDGET;
    while done < shown && next.len() < budget {
        match &t.messages[done] {
            Message::Assistant(a) => {
                let mut texts = 0u32;
                for (pi, part) in a.parts.iter().enumerate() {
                    let Part::Text { text } = part else { continue };
                    let n = texts;
                    texts += 1;
                    if text.trim().is_empty() && !(last_reply == Some(done) && pi + 1 == a.parts.len()) {
                        continue;
                    }
                    let kind = if outlines && (text.contains("```ui") || text.contains("~~~ui")) { Kind::Outline } else { Kind::Reply };
                    push(&mut next, &mut hash, &a.id, n, kind, text);
                }
            }
            Message::User { id, text } if !text.trim().is_empty() || !opencode_kit::model::user_comments(id).is_empty() => {
                let shown = s.bubble_text(id, text);
                push(&mut next, &mut hash, id, 0, Kind::Bubble, &shown);
            }
            _ => {}
        }
        done += 1;
    }
    if done < shown {
        return Copying::Running { next, done, hash };
    }
    next.len().hash(&mut hash);
    next.shape = hash.finish().max(1);
    Copying::Done(next)
}

/// Rows of `index` the timeline shows now (its last drawn frame).
fn rows_on_screen(s: &dyn FindHost, session: &str, index: &Index) -> Vec<usize> {
    let mut rows: Vec<usize> = s.visible_timeline_rows(session).iter().filter_map(|k| index.row_of(k)).collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// Lands the worker's index and matches: true when what shows changed.
pub(crate) fn land(s: &mut dyn FindHost, tx: &mut Tx) -> bool {
    let (index, matched) = {
        let f = s.find();
        (f.indexer.take(), f.matcher.take())
    };
    if index.is_none() && matched.is_none() {
        return false;
    }
    let landed = {
        let f = s.find_mut();
        if !f.is_open() {
            retire((index, matched));
            return false;
        }
        let mut changed = false;
        if let Some(index) = index
            && let Some(open) = f.open_mut()
        {
            retire(open.index.replace(index));
            open.indexing = Indexing::Idle;
            changed = true;
        }
        let Some((shape, query, results)) = matched else {
            return changed;
        };
        if let Some(open) = f.open_mut() {
            open.matching = Matching::Idle;
        }
        if f.query() != query {
            retire(results);
            return true;
        }
        let lit = f.lit().map(|(h, local)| (h.key.clone(), local));
        let Some(open) = f.open_mut() else {
            retire(results);
            return false;
        };
        let reveal = matches!(open.answer, Answer::Waiting { reveal: true, .. } | Answer::Partial { reveal: true, .. });
        retire(std::mem::replace(&mut open.answer, Answer::Complete { query, shape, results }));
        open.reveal_on_land = false;
        (reveal, lit)
    };
    // A target whose message is still to load keeps waiting, lighting the first match meanwhile.
    let waits = target_waits(s);
    let (reveal, lit) = landed;
    let should_seek = {
        let f = s.find_mut();
        let target = if waits {
            Target::First
        } else {
            f.open_mut().map_or(Target::First, |open| std::mem::take(&mut open.target))
        };
        let reveal = (reveal || target != Target::First) && !waits;
        let kept = match target {
            Target::Message(id) => f.results().and_then(|r| r.first_in_message(&id)).or(Some(0)),
            Target::First if reveal => Some(0),
            Target::First => lit.and_then(|(key, local)| f.results()?.place(&key, local)),
        };
        drop(f.state.step(Event::MatchesSynced { kept }));
        reveal && f.count() > 0
    };
    if should_seek {
        seek_active(s, tx);
    }
    true
}
