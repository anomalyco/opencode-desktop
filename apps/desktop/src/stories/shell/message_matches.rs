//! Storybook story for the palette's message matches (`Search/Message matches`): ⌘K searching
//! "stream", where sessions only their messages matched sit between one-line title matches, each
//! with its line of context and its other matches as "+n", one of them running.

use opencode_search::fulltext::{Kind, Snippet};
use rune::{El, Play, TextEdit, Tx};

use crate::{App, Route};
use crate::fulltext::{FULLTEXT, Row, Surface};
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::switcher::SWITCHER;

const QUERY: &str = "stream";
const MIN: i64 = 60_000;

/// Which row is highlighted: the first, the running one, or the 43-minute one.
const VARIANTS: &[(&str, usize)] = &[("Top row lit", 0), ("Running row lit", 1), ("Older row lit", 2), ("Session search (⇧⌘F)", 3), ("Home search", 4)];

/// The title matches: a session, its title, and how long ago it changed.
const TITLED: &[(&str, &str, i64)] =
    &[("ses_fx0001aaaaaaaaaaaaaaaaaa", "Fix flaky reconnect in the event stream", 0), ("ses_fx0005aaaaaaaaaaaaaaaaaa", "Stream backpressure question", 2 * 24 * 60)];

/// Sessions only their messages match: the session, minutes ago, what matched, its line of
/// context, whether it was cut before and after, and its other matches.
const FOUND: &[(&str, i64, Kind, &str, bool, bool, usize)] = &[
    ("ses_fx0006aaaaaaaaaaaaaaaaaa", 3, Kind::AssistantReply, "https://ci.example.com/acme/lumen/actions/runs/48213/jobs/upstream-logs?attempt=3&step=upload-artifacts&filter=warnings", false, true, 14),
    ("ses_fx0003aaaaaaaaaaaaaaaaaa", 26, Kind::UserPrompt, "Re-read the stream handler you mean (the one that batches title updates); drafting an update that keeps", false, true, 4),
    ("ses_fx0004aaaaaaaaaaaaaaaaaa", 27, Kind::UserPrompt, "...uff. I meant the one that streams with the reply, didn't it do some work on the keyboard order? The", true, true, 1),
    ("ses_fx0008aaaaaaaaaaaaaaaaaa", 43, Kind::AssistantReply, "in titles, duration format, \"stream\" casing, failed-row reasons, inline rename shifts and the", true, true, 5),
    ("ses_fx0007aaaaaaaaaaaaaaaaaa", 45, Kind::Draft, "before I touch the upload stream: 1. Go ahead and push the retry branch and open a draft PR", true, true, 0),
];

const KEYS: &[Trigger] = &[replay(setup)];

fn setup(s: &mut App, _: &mut Play<'_, App>) {
    init(s);
}

fn init(s: &mut App) {
    crate::open_scene(s);
    s.close_switcher(false);
    s.finish_stream();
    let now = *s.now_ms;
    for ses in s.sessions.iter_mut() {
        if let Some((_, title, ago)) = TITLED.iter().find(|(id, ..)| *id == ses.id) {
            (ses.title, ses.updated) = ((*title).to_owned(), now - ago * MIN - 20_000);
        } else if let Some((_, ago, ..)) = FOUND.iter().find(|(id, ..)| *id == ses.id) {
            ses.updated = now - ago * MIN - 20_000;
        }
    }
    let rows = FOUND
        .iter()
        .map(|&(session, _, kind, text, cut_before, cut_after, more)| {
            let marks = text.to_lowercase().find(QUERY).map(|at| at..at + QUERY.len()).into_iter().collect();
            let snippet = Snippet { text: text.to_owned(), marks, cut_before, cut_after };
            Row { session: session.to_owned(), kind, message: None, snippet, more, fallback: None }
        })
        .collect::<Vec<_>>();
    let mut tx = Tx::default();
    if *s.fixture_run.variant == 4 {
        s.kernel.slots.get_mut(&FULLTEXT).stage(Surface::Home, QUERY, rows);
        *s.route = Route::Home;
        s.focus_home_search(&mut tx);
        *s.search = TextEdit::new(QUERY);
        s.search.focused = true;
        s.poll_home_search(&mut tx);
        return;
    }
    s.kernel.slots.get_mut(&FULLTEXT).stage(Surface::Palette, QUERY, rows);
    s.fixture_run.fixture["find_files"] = serde_json::json!(["docs/streaming/backpressure.md", "crates/upload/src/stream.rs", "src/stream/reconnect.rs", "src/stream/mod.rs"]);
    if *s.fixture_run.variant == 3 {
        s.open_session_search(&mut tx);
    } else {
        s.open_switcher();
    }
    let sw = s.kernel.slots.get_mut(&SWITCHER);
    sw.search = TextEdit::new(QUERY);
    sw.search.focused = true;
    s.poll_switcher(&mut tx, false);
    let lit = match *s.fixture_run.variant {
        1 => Some(FOUND[0].0),
        2 => Some(FOUND[3].0),
        _ => None,
    };
    if let Some(id) = lit {
        s.kernel.slots.get_mut(&SWITCHER).activate(format!("session:{id}"));
    }
}

/// Home with its search panel, or an empty stage: the window's own overlay draws the palette,
/// as ⌘K does.
fn view(s: &App, p: &Pal) -> El {
    if *s.route == Route::Home {
        return rune::col().key("home-stage").w_full().h_full().child(crate::view::home(s, p, [720.0, 0.0], [0.0, 0.0]));
    }
    rune::col().w_full().h_full()
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Search/Message matches",
        scene: |s| {
            init(s);
            stage(s, view);
        },
        knobs: &[],
        keys: KEYS,
        variants: VARIANTS,
    },
]);
