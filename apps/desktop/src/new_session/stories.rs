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
    s.composer = rune::TextEdit::new(TEXT);
    s.composer.focused = true;
    tx.after(0.6, |s, tx| send(s, tx));
}

fn send(s: &mut App, tx: &mut Tx) {
    if matches!(s.route, Route::Draft(_)) {
        s.send(tx);
    }
}

/// As the server's refusal would: the session goes and the words come back.
fn fail(s: &mut App, tx: &mut Tx) {
    let Route::Session { id, .. } = s.route.clone() else { return };
    let Some(Message::User { id: prompt, text }) = s.transcripts.get(&id).and_then(|t| t.messages.iter().find(|m| matches!(m, Message::User { .. }))).cloned() else { return };
    s.playback.retain(|e| e.2["sessionID"] != id.as_str());
    let directory = s.draft_dir().to_owned();
    s.starts.step(&id, super::Event::Begin { directory, draft: None });
    s.unsent.note(crate::send_failed::Sent::new(&id, &prompt, crate::prompt_history::Entry::of(&rune::TextEdit::new(&text)), vec![], vec![], false));
    s.start_failed(&id, &prompt, "POST /api/session: Connection refused", tx);
}

fn view(s: &App, p: &Pal) -> El {
    let page = match &s.route {
        Route::Session { id, .. } => crate::view::session_page(s, p, id),
        _ => crate::view::draft_page(s, p, 820.0),
    };
    frame(p, "story-new-session").h_full().max_h(680.0).child(page)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Composer/New session", scene: |s| stage(s, view), knobs: &[], keys: KEYS, variants: &[] },
]);
