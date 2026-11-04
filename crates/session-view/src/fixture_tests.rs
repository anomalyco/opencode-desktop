use rune::{Cx, El, Env, Runtime, Tx, col};
use rune_knobs::Knobs;

use super::*;
use crate::palette::pal;
use crate::{TimelineHost, ViewState};

#[derive(Default)]
struct Host {
    knobs: Knobs,
    clock: f64,
    now_ms: i64,
    talk: Option<Transcript>,
    view: ViewState,
}

impl TimelineHost for Host {
    fn knobs(&self) -> &Knobs {
        &self.knobs
    }
    fn clock(&self) -> f64 {
        self.clock
    }
    fn now_ms(&self) -> i64 {
        self.now_ms
    }
    fn active_session_id(&self) -> Option<&str> {
        Some(SESSION_ID)
    }
    fn transcript(&self, session: &str) -> Option<&Transcript> {
        self.talk.as_ref().filter(|t| t.session == session)
    }
    crate::delegate_view_state!(view);
}

fn app(s: &Host, _cx: &mut Cx<Host>) -> El {
    let p = pal(true);
    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(p.bg_base)
        .child(crate::read_only(s, p, SESSION_ID, s.talk.as_ref()))
}

fn host_poll(s: &mut Host, now: f64, tx: &mut Tx) -> bool {
    s.clock = now;
    crate::poll(s, now, tx)
}

#[test]
fn read_only_host_renders_timeline_and_settles_idle() {
    let h = Host {
        now_ms: 1_700_000_000_000,
        talk: Some(transcript(1_700_000_000_000, false)),
        ..Default::default()
    };
    let keys = crate::timeline::row_keys(&h, h.talk.as_ref().unwrap());
    assert!(keys.iter().any(|k| k == "u-msg_sv_1"), "user row present: {keys:?}");
    assert!(keys.iter().any(|k| k.starts_with("c-")), "folded tool group present: {keys:?}");
    assert!(keys.iter().any(|k| k.starts_with("a-")), "subagent card present: {keys:?}");
    assert!(keys.iter().any(|k| k == "turn-actions"), "turn actions tail present: {keys:?}");

    let mut rt = Runtime::new(h, app, (960.0, 720.0), Env::new(true, 2.0));
    rt.poll = Some(host_poll);
    rt.frame(0.0);
    for _ in 0..60 {
        if !rt.frame(1.0 / 120.0) {
            break;
        }
    }
    assert_eq!(rt.scroll_at_bottom(&format!("scroll-{SESSION_ID}")), Some(true));
    let texts: Vec<String> = rt.elements().into_iter().filter_map(|e| e.text).collect();
    assert!(texts.iter().any(|t| t.contains("Read more")), "long prompt folded: {texts:?}");
    assert!(!host_poll(&mut rt.state, 2.0, &mut Tx::default()), "idle poll reports no change");
}
