//! Deterministic fixture transcripts for read-only session timeline stories and tests (`p255`).

use opencode_kit::model::{Transcript, parse_messages};
use serde_json::json;

pub const SESSION_ID: &str = "ses_seer_demo";

/// A multi-turn transcript exercising user bubbles, folded long prompts, tool groups, reasoning,
/// Markdown + code + `genui` + Mermaid diagram, subagent cards, and turn actions.
pub fn transcript(now_ms: i64, streaming: bool) -> Transcript {
    let completed = (!streaming).then_some(now_ms - 5_000);
    let long_prompt = (1..=15)
        .map(|i| format!("Line {i}: move the session timeline into crates/session-view with zero motion regressions."))
        .collect::<Vec<_>>()
        .join("\n");
    let messages = parse_messages(&json!({
        "data": [
            {
                "id": "msg_sv_1",
                "type": "user",
                "time": { "created": now_ms - 240_000 },
                "text": "Can Seer render an OpenCode session directly from crates/session-view?"
            },
            {
                "id": "msg_sv_2",
                "type": "assistant",
                "time": { "created": now_ms - 235_000, "completed": now_ms - 180_000 },
                "content": [
                    {
                        "type": "reasoning",
                        "text": "Check how apps/sketch/src/session_view feeds opencode_kit::model::Transcript and keep TimelineHost defaults minimal.",
                        "time": { "start": now_ms - 235_000, "end": now_ms - 232_000 }
                    },
                    {
                        "type": "tool",
                        "id": "prt_sv_1",
                        "name": "read",
                        "state": {
                            "status": "completed",
                            "input": { "filePath": "apps/sketch/src/session_view.rs" }
                        }
                    },
                    {
                        "type": "tool",
                        "id": "prt_sv_2",
                        "name": "grep",
                        "state": {
                            "status": "completed",
                            "input": { "pattern": "TimelineHost" }
                        }
                    },
                    {
                        "type": "text",
                        "text": "Yes — `session_view::read_only(s, &pal, id, transcript)` mounts the virtualized timeline, tool groups, Markdown, diagrams, scroll bar, and Jump to latest:\n\n```mermaid\ngraph LR\n  Kit[opencode-kit] --> View[crates/session-view]\n  View --> Desktop[apps/desktop]\n  View --> Seer[apps/sketch]\n```"
                    }
                ]
            },
            {
                "id": "msg_sv_3",
                "type": "user",
                "time": { "created": now_ms - 120_000 },
                "text": long_prompt
            },
            {
                "id": "msg_sv_4",
                "type": "assistant",
                "time": { "created": now_ms - 110_000, "completed": completed },
                "content": [
                    {
                        "type": "tool",
                        "id": "prt_sv_sub",
                        "name": "subagent",
                        "state": {
                            "status": if streaming { "running" } else { "completed" },
                            "title": "Verify motion audit scenarios",
                            "input": { "agent": "general", "description": "Verify motion audit scenarios" },
                            "metadata": { "sessionId": "ses_sv_child" }
                        }
                    },
                    {
                        "type": "text",
                        "text": "Long prompts fold cleanly to 8 lines with **Read more**, and subagent calls group into a live card:\n\n```rust\nlet timeline = session_view::read_only(s, &pal, id, Some(&transcript));\n```"
                    }
                ]
            }
        ]
    }));
    Transcript { session: SESSION_ID.to_owned(), messages }
}

#[cfg(test)]
mod tests {
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
        col().key("root").w_full().h_full().fill(p.bg_base).child(crate::read_only(s, p, SESSION_ID, s.talk.as_ref()))
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
}
