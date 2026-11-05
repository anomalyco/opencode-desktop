//! The sim against a real server (t1387): `recorded/<name>.json` is what OpenCode streamed for
//! one scenario (`scripts/interaction-fuzz/record.ts`, run on Drive with `real.sh record`).
//! Each test plays the same moves on the sim and compares the shape of the two streams: the
//! session's inbox, run, step, text and tool events in order, its helpers' events, and the
//! message types the transcript ends with.

use std::collections::HashMap;

use serde_json::Value;

use super::sim::{Kind, Msg, Sim};

struct Shape {
    parent: Vec<String>,
    helpers: Vec<String>,
}

/// One line per event that matters to the app, with prompts named by their first word.
fn shape<'a>(events: impl Iterator<Item = (&'a str, &'a Value)>, main: &str) -> Shape {
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut out = Shape { parent: vec![], helpers: vec![] };
    for (ty, d) in events {
        let Some(short) = ty.strip_prefix("session.") else { continue };
        let keep = ["inbox.", "execution.", "step.", "tool."].iter().any(|p| short.starts_with(p)) || matches!(short, "text.started" | "text.ended");
        if !keep {
            continue;
        }
        let sid = d["sessionID"].as_str().unwrap_or(main);
        let label = |d: &Value| -> String {
            if d["item"]["type"] == "synthetic" {
                return "synthetic".into();
            }
            if sid != main {
                return "prompt".into();
            }
            d["item"]["payload"]["text"].as_str().and_then(|t| t.split_whitespace().next()).unwrap_or("?").to_owned()
        };
        let extra = match short {
            "inbox.enqueued" => {
                let l = label(d);
                labels.insert(d["inboxID"].as_str().unwrap_or("").to_owned(), l.clone());
                format!(" {l} {}", d["item"]["delivery"].as_str().unwrap_or(""))
            }
            "inbox.delivered" | "inbox.cancelled" => format!(" {}", labels.get(d["inboxID"].as_str().unwrap_or("")).map_or("?", String::as_str)),
            "step.ended" => format!(" {}", d["finish"].as_str().unwrap_or("")),
            "step.failed" | "tool.failed" => format!(" {}", d["error"]["type"].as_str().unwrap_or("")),
            "execution.interrupted" => format!(" {}", d["reason"].as_str().unwrap_or("")),
            "tool.input.started" => format!(" {}", d["name"].as_str().unwrap_or("")),
            "tool.progress" | "tool.success" if d["metadata"]["sessionID"].is_string() => format!(" helper {}", d["metadata"]["status"].as_str().unwrap_or("")),
            _ => String::new(),
        };
        let line = format!("{short}{extra}");
        if sid == main { out.parent.push(line) } else { out.helpers.push(line) }
    }
    out
}

struct Recorded {
    shape: Shape,
    messages: Vec<String>,
}

fn recorded(name: &str) -> Recorded {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("src/interaction_fuzz/recorded/{name}.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(v["stuck"].is_null(), "{name} didn't settle when it was recorded");
    let main = v["sessionID"].as_str().unwrap();
    let events = v["events"].as_array().unwrap().iter().map(|e| (e["type"].as_str().unwrap_or(""), &e["data"]));
    let messages = v["messages"]["data"].as_array().unwrap().iter().map(|m| m["type"].as_str().unwrap_or("").to_owned()).collect();
    Recorded { shape: shape(events, main), messages }
}

fn sim(script: &[(Kind, f64)]) -> Sim {
    let template = opencode_kit::model::SessionInfo { id: "ses_fuzzrec0000000000000000".into(), ..Default::default() };
    let mut s = Sim::new(5, template);
    s.script = script.iter().copied().collect();
    s
}

/// The sim's stream and transcript since it started (past its two seeded messages).
fn played(s: &Sim) -> Recorded {
    let events = s.out.iter().map(|(_, ty, d)| (ty.as_str(), d));
    let messages = s.msgs[2..]
        .iter()
        .map(|m| match m {
            Msg::User { .. } => "user",
            Msg::Asst { .. } => "assistant",
            Msg::Synthetic { .. } => "synthetic",
            Msg::Idle { .. } => "idle",
        })
        .map(str::to_owned)
        .collect();
    Recorded { shape: shape(events, &s.sid), messages }
}

fn check(name: &str, s: &mut Sim) {
    s.advance(s.t + 60.0);
    let (real, sim) = (recorded(name), played(s));
    let show = |v: &[String]| v.join("\n  ");
    assert_eq!(sim.shape.parent, real.shape.parent, "{name}: the session's events differ\nsim:\n  {}\nreal:\n  {}", show(&sim.shape.parent), show(&real.shape.parent));
    assert_eq!(sim.shape.helpers, real.shape.helpers, "{name}: the helpers' events differ\nsim:\n  {}\nreal:\n  {}", show(&sim.shape.helpers), show(&real.shape.helpers));
    assert_eq!(sim.messages, real.messages, "{name}: the transcripts differ");
}

const HELD: f64 = 1e6;

#[test]
fn plain() {
    let mut s = sim(&[(Kind::Reply, 0.5)]);
    s.admit("msg_p1", "P1Q hello", false);
    check("plain", &mut s);
}

#[test]
fn steer() {
    let mut s = sim(&[(Kind::Reply, 0.6), (Kind::Reply, 0.5)]);
    s.admit("msg_h2", "H2Q wait", false);
    s.advance(0.3);
    s.admit("msg_p3", "P3Q and this", false);
    check("steer", &mut s);
}

#[test]
fn queue() {
    let mut s = sim(&[(Kind::Reply, 0.6), (Kind::Reply, 0.5)]);
    s.admit("msg_h4", "H4Q wait", false);
    s.advance(0.3);
    s.admit("msg_p5", "P5Q later", true);
    check("queue", &mut s);
}

#[test]
fn stop() {
    let mut s = sim(&[(Kind::Reply, HELD)]);
    s.admit("msg_h6", "H6Q wait", false);
    s.advance(0.3);
    assert!(s.interrupt());
    check("stop", &mut s);
}

#[test]
fn stop_then_prompt() {
    let mut s = sim(&[(Kind::Reply, HELD), (Kind::Reply, 0.5)]);
    s.admit("msg_h7", "H7Q wait", false);
    s.advance(0.3);
    assert!(s.interrupt());
    s.admit("msg_p8", "P8Q right after", false);
    check("stop-then-prompt", &mut s);
}

#[test]
fn stop_with_queued() {
    let mut s = sim(&[(Kind::Reply, HELD), (Kind::Reply, 0.5), (Kind::Reply, 0.5)]);
    s.admit("msg_h10", "H10Q wait", false);
    s.advance(0.3);
    s.admit("msg_p11", "P11Q later", true);
    s.advance(0.5);
    assert!(s.interrupt());
    s.advance(1.5);
    s.admit("msg_p12", "P12Q wakes it", false);
    check("stop-with-queued", &mut s);
}

#[test]
fn tool() {
    let mut s = sim(&[(Kind::Tool, 1.0), (Kind::Reply, 0.5)]);
    s.admit("msg_t13", "T13Q run it", false);
    check("tool", &mut s);
}

#[test]
fn subagent() {
    let mut s = sim(&[(Kind::Subagent, 1.0), (Kind::Reply, 0.5)]);
    s.admit("msg_a14", "A14Q delegate", false);
    check("subagent", &mut s);
}

#[test]
fn background_helper() {
    let mut s = sim(&[(Kind::Background, 0.8), (Kind::Reply, 0.3), (Kind::Reply, 0.3)]);
    s.admit("msg_b15", "B15Q delegate", false);
    check("background-helper", &mut s);
}

#[test]
fn stop_during_tool() {
    let mut s = sim(&[(Kind::Tool, HELD)]);
    s.admit("msg_w16", "W16Q run it", false);
    s.advance(0.4);
    assert!(s.interrupt());
    check("stop-during-tool", &mut s);
}

#[test]
fn helper_wakes_idle_parent() {
    let mut s = sim(&[(Kind::Background, 1.5), (Kind::Reply, 0.3), (Kind::Reply, 0.3)]);
    s.admit("msg_c18", "C18Q delegate", false);
    check("helper-wakes-idle-parent", &mut s);
}

#[test]
fn queue_while_idle() {
    let mut s = sim(&[(Kind::Reply, 0.5)]);
    s.admit("msg_p19", "P19Q queued", true);
    check("queue-while-idle", &mut s);
}

#[test]
fn stop_twice() {
    let mut s = sim(&[(Kind::Reply, HELD)]);
    s.admit("msg_h20", "H20Q wait", false);
    s.advance(0.3);
    assert!(s.interrupt());
    assert!(!s.interrupt());
    s.advance(0.6);
    assert!(!s.interrupt());
    check("stop-twice", &mut s);
}

#[test]
fn stop_while_idle() {
    let mut s = sim(&[(Kind::Reply, 0.5)]);
    s.admit("msg_p21", "P21Q hello", false);
    s.advance(1.5);
    assert!(!s.interrupt());
    check("stop-while-idle", &mut s);
}

#[test]
fn stop_during_subagent() {
    let mut s = sim(&[(Kind::Subagent, HELD)]);
    s.admit("msg_s17", "S17Q delegate", false);
    s.advance(0.6);
    assert!(s.interrupt());
    check("stop-during-subagent", &mut s);
}
