//! The storybook's Long prompt story (Timeline › Long prompt): a pasted call transcript sent
//! from the composer flies up and lands folded to its first 8 lines behind Read more (`fold.rs`).
//! O opens or closes it, S sends another, R replays. The Expanded variant opens it once it lands; Fold off
//! turns the setting off; Steer and Queued send it while a run works.

use rune::{Play, TextEdit, Tx};

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'o', label: "Read more / Show less", run: |s, tx| flip(s, tx) },
    Trigger { key: 's', label: "Send another", run: |s, tx| send(s, tx) },
];
pub const VARIANTS: &[(&str, usize)] = &[("Folded", 0), ("Expanded", 1), ("Fold off", 2), ("Steer", 3), ("Queued", 4)];

/// A long prompt: a call's notes pasted whole, 26 lines.
pub const LONG: &str = "Notes from the call about the reconnect work, pasted as they came:
- the socket drops when the laptop sleeps and the client waits the full 30 s before retrying
- on a hotel network the first retry often fails too, so the backoff doubles straight away
- we agreed the first retry should go at once, then back off from 1 s
- cap the backoff at 30 s, as now, but add jitter so a fleet of clients doesn't retry in step
- jitter of plus or minus 20% was the number that came up
- every retry should log its attempt number and the delay it chose
- the sidebar shows Offline after the second failed attempt, not the first
- when the stream comes back the sidebar should recover without a reload
- the timeline should keep its scroll position across a reconnect
- messages sent while offline wait in the queue and go in order once it's back
- if one of them fails, the rest keep waiting rather than skipping ahead
- we want a test that drops the socket twice in a row and checks the order
- and one that sleeps the machine on a fake clock and checks the first retry is immediate
- keep one slow end-to-end test on real timers so a regression shows up somewhere
- the VPN case: the interface changes under us, which looks like a drop with no error
- treat an interface change as a drop and retry at once
- don't show the Offline banner for drops shorter than a second
- the status bar can show the attempt count while it retries, faint
- nothing in the composer should change while it's offline; sending just queues
- after three hours offline, stop retrying and show a Reconnect button instead
- the button retries once and goes back to the normal backoff if that fails
- write the PR description with a before and after of the sidebar
- and a short clip of the drop and recovery on the fake clock
- no changes to the server for any of this
That's everything from the call; start with the backoff and the tests.";

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.settings.fold_user = s.variant != 2;
    s.composer = TextEdit::new(LONG);
    if s.variant >= 3 {
        // While a run works the prompt waits: as a Pending steer, or in the queue.
        s.running.insert(sid(s));
    }
    tx.after(0.6, |s, tx| send(s, tx));
    if s.variant == 1 {
        tx.after(2.2, |s, tx| flip(s, tx));
    }
}

fn send(s: &mut App, tx: &mut Tx) {
    s.composer = TextEdit::new(LONG);
    s.submit(s.variant == 4, tx);
}

/// Opens or closes the last long prompt.
fn flip(s: &mut App, tx: &mut Tx) {
    let Some(t) = s.transcripts.get(&sid(s)) else { return };
    let last = t.messages.iter().rev().find_map(|m| match m {
        opencode_kit::model::Message::User { id, text } if text.lines().count() > crate::fold::OVER => Some(id.clone()),
        _ => None,
    });
    if let Some(id) = last {
        crate::fold::flip(s, &id, false, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Long prompt", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);
