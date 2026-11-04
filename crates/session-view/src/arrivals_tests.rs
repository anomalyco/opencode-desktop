use opencode_kit::model::{Transcript, parse_messages};
use rune::Tx;
use rune_knobs::Knobs;
use serde_json::json;

use super::*;

fn talk(ids: &[(&str, &str)]) -> Transcript {
    let data: Vec<_> = ids.iter().map(|(id, ty)| match *ty {
        "user" => json!({"id": id, "type": "user", "time": {"created": 1}, "text": "hi"}),
        _ => json!({"id": id, "type": "assistant", "time": {"created": 1, "completed": 2}, "content": [{"type": "text", "text": "ok"}]}),
    }).collect();
    Transcript { session: "ses_a".into(), messages: parse_messages(&json!({ "data": data })) }
}

#[test]
fn a_prompt_past_the_old_end_glows() {
    let before = talk(&[("m1", "user"), ("m2", "assistant")]);
    let seen = Seen::of(&before);
    let after = talk(&[("m1", "user"), ("m2", "assistant"), ("m3", "user")]);
    let mut a = Arrivals::default();
    assert!(a.landed(seen.as_ref(), &after, 5.0));
    assert_eq!(a.glow_at("m3"), Some(5.0));
    assert_eq!(a.glow_at("m1"), None);
}

#[test]
fn older_history_and_a_first_load_stay_still() {
    let before = talk(&[("m3", "user")]);
    let older = talk(&[("m1", "user"), ("m2", "assistant"), ("m3", "user")]);
    let mut a = Arrivals::default();
    assert!(!a.landed(Seen::of(&before).as_ref(), &older, 1.0), "history loaded above doesn't glow");
    assert!(!a.landed(None, &older, 1.0), "nor does a first load");
    assert_eq!(a.glow_at("m1"), None);
}

#[test]
fn a_first_load_settles_and_a_glow_keeps_frames_coming_until_it_ends() {
    let k = Knobs::default();
    let mut a = Arrivals::default();
    a.landed(None, &talk(&[("m1", "user")]), 0.0);
    assert!(a.settle, "a first load lands in place");
    assert!(!a.poll(&k, 0.0, &mut Tx::default()));
    assert!(!a.settle, "once");
    a.landed(Seen::of(&talk(&[("m1", "user")])).as_ref(), &talk(&[("m1", "user"), ("m2", "user")]), 1.0);
    assert!(!a.settle, "an arrival animates");
    assert!(a.poll(&k, 1.1, &mut Tx::default()), "a glowing prompt redraws");
    assert!(!a.poll(&k, 1.0 + crate::knobs::glow_secs(&k) + 0.1, &mut Tx::default()), "and stops once its glow ends");
    assert_eq!(a.glow_at("m2"), None);
}
