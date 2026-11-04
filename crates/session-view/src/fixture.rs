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
                        "text": "Yes — `TimelineHost::with_transcript` borrows the transcript in place so virtual rows mount without cloning:\n\n```rust\nsession_view::read_only(s, &pal, id, transcript)\n```"
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
                        "text": "Long prompts fold to 8 lines with **Read more**, subagent calls group into a card, and diagrams open full-size:\n\n```mermaid\ngraph LR\n  Kit[opencode-kit] --> View[session-view]\n  View --> Desktop[opencode-desktop]\n  View --> Seer[seer-sketch]\n```"
                    }
                ]
            }
        ]
    }));
    Transcript { session: SESSION_ID.to_owned(), messages }
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
