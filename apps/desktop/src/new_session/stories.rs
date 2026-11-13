//! Composer › New session: the new-session page with a prompt typed, sent 0.6 s in. The page
//! switches at once to the new session, the prompt rising into place with Working under it,
//! and the reply streams in. R replays, S sends now, F fails the start and goes back.
use opencode_kit::model::Message;
use rune::{El, Play, Tx};

use crate::palette::Pal;
use crate::stories::{Story, Trigger, frame, replay, stage};
use crate::{App, Route};

const TEXT: &str = "Find out why the sidebar flickers when the stream reconnects";

const KEYS: &[Trigger] = &[replay(open), Trigger { key: 's', label: "Send", run: |s, tx| send(s, tx) }, Trigger { key: 'f', label: "Fail the start", run: |s, tx| fail(s, tx) }];

fn open(s: &mut App, tx: &mut Play<App>) {
    s.new_session(tx);
    *s.composer = rune::TextEdit::new(TEXT);
    s.composer.focused = true;
    tx.after(0.6, |s, tx| send(s, tx));
}

fn send(s: &mut App, tx: &mut Tx) {
    if matches!(*s.route, Route::Draft(_)) {
        s.send(tx);
    }
}

/// As the server's refusal would: the session goes and the words come back.
fn fail(s: &mut App, tx: &mut Tx) {
    let Route::Session { id, .. } = (*s.route).clone() else { return };
    let Some(Message::User { id: prompt, text }) = s.transcripts.get(&id).and_then(|t| t.messages.iter().find(|m| matches!(m, Message::User { .. }))).cloned() else { return };
    s.playback.retain(|e| e.2["sessionID"] != id.as_str());
    let directory = s.draft_dir().to_owned();
    s.starts.starts.insert(id.clone(), super::Start::Creating { directory, draft: None, created: false });
    s.unsent.note(crate::send_failed::Sent::new(&id, &prompt, crate::prompt_history::Entry::of(&rune::TextEdit::new(&text)), vec![], vec![], false));
    s.start_failed(&id, &prompt, &opencode_kit::wire_error::WireError::new("POST", "/api/session", opencode_kit::wire_error::WireKind::Unreachable("Connection refused".into())), tx);
}

fn view(s: &App, p: &Pal) -> El {
    let page = match &*s.route {
        Route::Session { id, .. } => crate::view::session_page(s, p, id),
        _ => crate::view::draft_page(s, p, 820.0),
    };
    frame(p, "story-new-session").h_full().max_h(680.0).child(page)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Composer/New session", scene: |s| stage(s, view), knobs: &[], keys: KEYS, variants: &[] },
]);

/// The `start-*` scenes: a session just sent from the new-session page, held in one of its
/// start's phases, its status line under the prompt (Desktop's `PreparingSession`).
fn start_scene(s: &mut App, worktree: bool) {
    crate::scenes::open_scene(s);
    let directory = match &*s.route {
        Route::Session { id, .. } => s.session(id).map(|i| i.directory.clone()).unwrap_or_default(),
        _ => String::new(),
    };
    let (id, prompt) = ("ses_zzzzstartscene".to_owned(), "msg_zzzzstartscene".to_owned());
    s.sessions.insert(0, opencode_kit::model::SessionInfo { id: id.clone(), directory: directory.clone(), ..Default::default() });
    s.transcripts.insert(id.clone(), opencode_kit::model::Transcript { session: id.clone(), messages: vec![Message::User { id: prompt.clone(), text: TEXT.into() }] });
    *s.route = Route::here(id.clone());
    let start = if worktree {
        super::Start::Worktree { attempt: rune::machine::Attempt(1), draft: None, held: super::Held { id: prompt.clone() }, due: f64::INFINITY }
    } else {
        super::Start::Creating { directory, draft: None, created: false }
    };
    s.starts.starts.insert(id, start);
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("start-worktree", |s| start_scene(s, true)),
    ("start-session", |s| start_scene(s, false)),
]);
