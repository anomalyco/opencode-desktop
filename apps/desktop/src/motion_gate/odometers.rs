//! Odometer cells: live numbers roll only their changed digits through a soft edge
//! (`odometer.rs`), and a number that snaps in place is a `text-swap`.

use opencode_kit::model::Message;
use rune::{Key, KeyEvent, Mods};

use super::{Run, SID};
use crate::App;

/// A subagent's card time ticks from `secs` to the next second: 9s → 10s widens by a digit,
/// 59s → 1m 00s changes format. The card lands, the second turns 0.7 s in, and the run ends
/// still, before the next one.
pub fn time(r: &mut Run, secs: i64) {
    r.act("an agent starts", |rt| rt.update(move |s, tx| {
        crate::agent_stories::spawn(s, tx);
        let now = s.now_ms;
        if let Some(c) = s.sessions.iter_mut().rev().find(|x| x.parent.as_deref() == Some(SID)) {
            c.created = now - secs * 1000 - 300;
        }
    }));
    r.play(1.6);
}

/// The header's context and cost rise as a step finishes, then rise past a round number.
pub fn usage(r: &mut Run) {
    r.act("a step finishes", |rt| rt.update(|s, _| spend(s, 0.07, 1_400)));
    r.play(0.8);
    r.act("a big step finishes", |rt| rt.update(|s, _| spend(s, 9.96, 98_000)));
    r.play(1.0);
}

fn spend(s: &mut App, cost: f64, tokens: u64) {
    if let Some(i) = s.sessions.iter_mut().find(|x| x.id == SID) {
        i.cost += cost;
    }
    let last = s.transcripts.get_mut(SID).and_then(|t| {
        t.messages.iter_mut().rev().find_map(|m| match m {
            Message::Assistant(a) if a.tokens.context() > 0 => Some(a),
            _ => None,
        })
    });
    if let Some(a) = last {
        a.tokens.input += tokens;
        a.cost += cost;
    }
}

/// The question dock's "1 of 4 questions" counts on as each is answered.
pub fn questions(r: &mut Run) {
    for (ty, mut d) in crate::question_stories::asked(4) {
        d["sessionID"] = SID.into();
        r.event(ty, d);
    }
    r.play(1.5);
    for _ in 0..2 {
        r.act("1", |rt| rt.key(KeyEvent { key: Key::Text("1".into()), mods: Mods::default() }));
        r.play(0.8);
    }
}

/// Four prompts wait, so the queue shows its count, and a fifth joins.
pub fn queue(r: &mut Run) {
    let add = |s: &mut App, k: usize| {
        s.inboxes.entry(SID.into()).or_default().upsert(opencode_kit::model::InboxItem {
            id: format!("msg_zzodometer{k}"),
            text: format!("Then check step {k}"),
            delivery: opencode_kit::model::Delivery::Queue,
            files: vec![],
        });
    };
    r.act("four queued", |rt| rt.update(move |s, _| (0..4).for_each(|k| add(s, k))));
    r.play(1.0);
    r.act("a fifth", |rt| rt.update(move |s, _| add(s, 4)));
    r.play(1.0);
}
