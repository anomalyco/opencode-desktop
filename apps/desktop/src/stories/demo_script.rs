//! The demo's story (`demo.rs`): one agent turn on a made-up project, as the `/api/event`
//! payloads a server would stream, each at its time from the start. Times come from a seeded
//! generator, so every run plays the same.
//!
//! Besides server events it has a few of the demo's own (`demo.*`): typing into the composer,
//! sending, a subagent's session appearing, and picking a question's answer.

use serde_json::{Value, json};

/// The demo session's ids for replay `run`, so a replay never meets an earlier run's state.
pub struct Ids {
    pub session: String,
    pub run: u32,
}

impl Ids {
    pub fn new(run: u32) -> Ids {
        Ids { session: format!("ses_demo{run:02}aaaaaaaaaaaaaaaaaa"), run }
    }

    pub fn child(&self, k: usize) -> String {
        format!("ses_demo{:02}child{k}aaaaaaaaaaaa", self.run)
    }

    fn message(&self, n: usize) -> String {
        format!("msg_zzzzzdemo{:02}{n:03}", self.run)
    }
}

pub const TITLE: &str = "Add dark mode to the settings page";
pub const PROMPT: &str = "Add dark mode to the settings page. Follow the system theme unless I pick one";
pub const STEER: &str = "Also check the docs viewer, its code blocks look wrong in dark";

/// One scheduled event: when (seconds from the start), its type and payload.
pub type Event = (f64, String, Value);

struct Script<'a> {
    ids: &'a Ids,
    t: f64,
    seed: u64,
    steps: usize,
    out: Vec<Event>,
}

impl Script<'_> {
    /// A uniform number in [0, 1) from the script's own generator.
    fn rand(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn wait(&mut self, secs: f64) {
        self.t += secs;
    }

    fn at(&mut self, t: f64) {
        self.t = self.t.max(t);
    }

    fn ev(&mut self, ty: &str, mut data: Value) {
        if data["sessionID"].is_null() {
            data["sessionID"] = self.ids.session.clone().into();
        }
        self.out.push(((self.t * 1000.0).round() / 1000.0, ty.to_owned(), data));
    }

    /// Typed into the composer at a person's pace: uneven keystrokes, a beat after a comma.
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let gap = 0.045 + self.rand() * 0.06 + if matches!(c, ',' | '.') { 0.22 } else if c == ' ' { self.rand() * 0.05 } else { 0.0 };
            self.wait(gap);
            self.ev("demo.type", json!({ "text": c.to_string() }));
        }
    }

    fn step(&mut self) -> String {
        self.steps += 1;
        let id = self.ids.message(self.steps);
        self.ev("session.step.started", json!({ "assistantMessageID": id, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" } }));
        id
    }

    fn end_step(&mut self, mid: &str, cost: f64, context: u64) {
        let tokens = json!({ "input": 8, "output": context / 40, "reasoning": 160, "cache": { "read": context, "write": 1200 } });
        self.ev("session.step.ended", json!({ "assistantMessageID": mid, "finish": "tool-calls", "cost": cost, "tokens": tokens }));
    }

    /// Words as a model streams them: one to three per delta, uneven gaps, faster in code.
    fn stream(&mut self, kind: &str, mid: &str, ordinal: usize, text: &str) {
        let base = json!({ "assistantMessageID": mid, "ordinal": ordinal });
        let with = |extra: Value| {
            let mut d = base.clone();
            d.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            d
        };
        self.ev(&format!("session.{kind}.started"), base.clone());
        let mut pieces = Vec::new();
        let mut buf = String::new();
        for c in text.chars() {
            buf.push(c);
            if matches!(c, ' ' | '\n') && self.rand() < 0.55 {
                pieces.push(std::mem::take(&mut buf));
            }
        }
        if !buf.is_empty() {
            pieces.push(buf);
        }
        let mut code = false;
        for p in pieces {
            if p.contains("```") {
                code = !code;
            }
            let gap = if code { 0.016 + self.rand() * 0.028 } else { 0.03 + self.rand() * 0.05 };
            self.wait(gap);
            self.ev(&format!("session.{kind}.delta"), with(json!({ "delta": p })));
        }
        self.wait(0.15);
        self.ev(&format!("session.{kind}.ended"), with(json!({ "text": text })));
    }

    /// A tool call: written, run for `secs`, then its result.
    fn tool(&mut self, mid: &str, name: &str, input: Value, secs: f64, metadata: Value) {
        let id = format!("call_demo{:02}_{}_{}", self.ids.run, name, self.out.len());
        self.ev("session.tool.input.started", json!({ "assistantMessageID": mid, "id": id, "name": name }));
        let r = self.rand();
        self.wait(0.12 + r * 0.08);
        self.ev("session.tool.called", json!({ "assistantMessageID": mid, "id": id, "name": name, "input": input, "executed": true }));
        self.wait(secs);
        self.ev("session.tool.success", json!({ "assistantMessageID": mid, "id": id, "content": [{ "type": "text", "text": "…" }], "metadata": metadata, "executed": true }));
        let r = self.rand();
        self.wait(0.15 + r * 0.15);
    }
}

fn edit(file: &str, before: &str, after: &str) -> Value {
    let diff = format!("--- {file}\n+++ {file}\n@@\n-{}\n+{}\n", before.replace('\n', "\n-"), after.replace('\n', "\n+"));
    json!({ "diff": diff, "filediff": { "file": file, "before": before, "after": after, "additions": after.lines().count(), "deletions": before.lines().count() } })
}

const DIR: &str = "/Users/demo/code/lumen";

/// The whole turn for `ids`, ordered by time.
pub fn events(ids: &Ids) -> Vec<Event> {
    let mut s = Script { ids, t: 0.0, seed: 0x5eed_da4c, steps: 0, out: Vec::new() };
    let p = |f: &str| format!("{DIR}/{f}");

    // Kit types the prompt and sends it; the run starts.
    s.at(1.0);
    s.type_text(PROMPT);
    s.wait(0.6);
    s.ev("demo.send", json!({}));
    s.wait(0.35);
    s.ev("session.execution.started", json!({}));
    s.wait(0.1);
    let m1 = s.step();
    s.wait(0.4);
    s.stream("reasoning", &m1, 0, "Settings builds its colors from the theme tokens. I should see how the tokens are defined, whether anything reads prefers-color-scheme yet, and where settings keep their preferences.");
    s.ev("session.renamed", json!({ "title": TITLE }));
    s.wait(0.3);
    s.tool(&m1, "grep", json!({ "pattern": "prefers-color-scheme|colorScheme" }), 0.5, json!({ "matches": 2 }));
    s.tool(&m1, "read", json!({ "filePath": p("src/theme/tokens.ts") }), 0.45, json!({}));
    s.tool(&m1, "read", json!({ "filePath": p("src/settings/SettingsPage.tsx") }), 0.5, json!({}));
    s.tool(&m1, "read", json!({ "filePath": p("src/settings/store.ts") }), 0.4, json!({}));
    s.end_step(&m1, 0.06, 18_400);

    // The plan, with Markdown and a code block, then the edits and a test run.
    s.wait(0.25);
    let m2 = s.step();
    s.wait(0.3);
    s.stream("text", &m2, 0, PLAN);
    s.wait(0.3);
    let steer_from = s.t + 1.6;
    s.tool(&m2, "edit", json!({ "filePath": p("src/theme/tokens.ts") }), 1.0, edit("src/theme/tokens.ts", "export const colors = {", "export const light = {\n…\nexport const dark = {"));
    s.tool(&m2, "write", json!({ "filePath": p("src/theme/applyTheme.ts") }), 0.8, edit("src/theme/applyTheme.ts", "", "export type Theme = \"system\" | \"light\" | \"dark\""));
    s.tool(&m2, "edit", json!({ "filePath": p("src/settings/SettingsPage.tsx") }), 1.2, edit("src/settings/SettingsPage.tsx", "<Row label=\"Language\">", "<Row label=\"Appearance\">\n  <Segmented options={THEMES} />\n</Row>\n<Row label=\"Language\">"));
    s.tool(&m2, "shell", json!({ "command": "bun test src/settings", "description": "Run the settings tests" }), 3.2, json!({ "output": "bun test v1.3.2\n\nsrc/settings/store.test.ts:\n✓ remembers the theme [1.20ms]\n✓ falls back to system [0.41ms]\n\n 9 pass\n 0 fail\nRan 9 tests across 3 files. [212.00ms]", "exit": 0 }));
    let m2_end = s.t;
    s.end_step(&m2, 0.11, 24_900);

    // Meanwhile Kit types a second message; it waits as a steer, then the run takes it up.
    let mut typing = Script { ids, t: steer_from, seed: 0x7e3a_11ce, steps: 0, out: Vec::new() };
    typing.type_text(STEER);
    typing.wait(0.5);
    typing.ev("demo.send", json!({}));
    let sent = typing.t;
    s.out.extend(typing.out);
    s.at(m2_end.max(sent + 2.2) + 0.3);
    s.ev("session.inbox.delivered", json!({ "next": "steer" }));
    s.wait(0.5);
    let m3 = s.step();
    s.wait(0.3);
    s.stream("text", &m3, 0, "Good catch: the docs viewer hard-codes its syntax colors. I'll run two agents at once, one moving the docs viewer onto the theme tokens and one sweeping the rest of the app for hard-coded colors.");
    s.wait(0.3);

    // Two subagents in parallel, each with its own steps and running time.
    let agents = [
        ("Move the docs viewer onto theme tokens", "Docs viewer now reads its syntax colors from the theme; 3 files changed.", &["Read CodeBlock.tsx", "Grep #[0-9a-f]{6}", "Edit CodeBlock.tsx", "Edit highlight.css", "Shell bun test src/docs"][..], 13.0),
        ("Find hard-coded colors", "Replaced 11 hard-coded colors in 6 files with theme tokens.", &["Grep #[0-9a-f]{6}|rgb\\(", "Read Sidebar.tsx", "Edit Sidebar.tsx", "Read Toast.tsx", "Edit Toast.tsx", "Edit Badge.tsx", "Shell bun run lint"][..], 18.0),
    ];
    let calls: Vec<String> = (0..agents.len()).map(|k| format!("call_demo{:02}_agent{k}", ids.run)).collect();
    let start = s.t;
    for (k, (title, ..)) in agents.iter().enumerate() {
        let child = ids.child(k);
        s.ev("session.tool.input.started", json!({ "assistantMessageID": m3, "id": calls[k], "name": "subagent" }));
        s.wait(0.15);
        s.ev("demo.child", json!({ "id": child, "title": title }));
        s.ev("session.tool.called", json!({ "assistantMessageID": m3, "id": calls[k], "name": "subagent", "input": { "agent": "general", "description": title, "prompt": title } }));
        s.ev("session.tool.progress", json!({ "assistantMessageID": m3, "id": calls[k], "metadata": { "sessionID": child, "status": "running" } }));
        s.ev("session.execution.started", json!({ "sessionID": child }));
        s.wait(0.25);
    }
    let mut ends = Vec::new();
    for (k, (_, answer, steps, secs)) in agents.iter().enumerate() {
        let child = ids.child(k);
        let reply = format!("msg_zzzzzdemo{:02}c{k}", ids.run);
        let mut c = Script { ids, t: start + 0.4 + k as f64 * 0.25, seed: 0xc4_11d0 + k as u64, steps: 0, out: Vec::new() };
        c.ev("session.step.started", json!({ "sessionID": child, "assistantMessageID": reply }));
        let gap = secs / (steps.len() as f64 + 0.5);
        for (n, step) in steps.iter().enumerate() {
            let r = c.rand();
            c.wait(gap * (0.7 + r * 0.6));
            let (verb, target) = step.split_once(' ').unwrap_or((step, ""));
            let (name, field) = match verb {
                "Read" => ("read", "filePath"),
                "Edit" => ("edit", "filePath"),
                "Grep" => ("grep", "pattern"),
                _ => ("shell", "command"),
            };
            let call = format!("call_demo{:02}_c{k}_{n}", ids.run);
            c.ev("session.tool.called", json!({ "sessionID": child, "assistantMessageID": reply, "id": call, "name": name, "input": { field: target } }));
        }
        c.at(start + secs);
        c.ev("session.text.started", json!({ "sessionID": child, "assistantMessageID": reply, "ordinal": 0 }));
        c.ev("session.text.ended", json!({ "sessionID": child, "assistantMessageID": reply, "ordinal": 0, "text": answer }));
        c.ev("session.step.ended", json!({ "sessionID": child, "assistantMessageID": reply, "finish": "stop", "cost": 0.04 }));
        c.ev("session.execution.succeeded", json!({ "sessionID": child }));
        c.wait(0.1);
        c.ev("session.tool.success", json!({ "assistantMessageID": m3, "id": calls[k], "metadata": { "sessionID": child, "status": "completed" }, "content": [{ "type": "text", "text": answer }] }));
        ends.push(c.t);
        s.out.extend(c.out);
    }
    s.at(ends.iter().cloned().fold(0.0, f64::max) + 0.3);
    s.end_step(&m3, 0.09, 31_200);

    // A question, answered from the dock.
    s.wait(0.3);
    let m4 = s.step();
    s.wait(0.3);
    s.stream("text", &m4, 0, "Both agents are done. One choice before I wire up the toggle:");
    s.wait(0.3);
    let call = format!("call_demo{:02}_question", ids.run);
    let form = format!("frm_demo{:02}", ids.run);
    let options = ["On this device (Recommended)", "Synced to my account"];
    let question = "Where should the theme choice be saved?";
    s.ev("session.tool.called", json!({ "assistantMessageID": m4, "id": call, "name": "question", "input": { "questions": [{ "header": "Theme", "question": question, "options": options.iter().map(|o| json!({ "label": o, "description": "" })).collect::<Vec<_>>() }] } }));
    s.ev("form.created", json!({ "id": form, "title": "Questions", "metadata": { "kind": "question", "tool": { "messageID": m4, "id": call } }, "fields": [{ "key": "q0", "type": "string", "title": "Theme", "description": question, "custom": true, "options": options.iter().map(|o| json!({ "value": o, "label": o })).collect::<Vec<_>>() }] }));
    s.wait(4.5);
    s.ev("demo.pick", json!({ "key": "1" }));
    s.wait(0.8);
    s.ev("form.replied", json!({ "id": form }));
    s.ev("session.tool.success", json!({ "assistantMessageID": m4, "id": call, "metadata": { "answers": [[options[0]]] } }));
    s.wait(0.2);
    s.end_step(&m4, 0.05, 33_800);

    // The last edit, the full test run, and the summary; the run ends.
    s.wait(0.3);
    let m5 = s.step();
    s.wait(0.3);
    s.tool(&m5, "edit", json!({ "filePath": p("src/settings/store.ts") }), 0.9, edit("src/settings/store.ts", "language: \"en\",", "language: \"en\",\ntheme: \"system\" as Theme,"));
    s.tool(&m5, "shell", json!({ "command": "bun test", "description": "Run all tests" }), 3.4, json!({ "output": "bun test v1.3.2\n\n 48 pass\n 0 fail\nRan 48 tests across 17 files. [1.31s]", "exit": 0 }));
    s.stream("text", &m5, 0, SUMMARY);
    s.wait(0.3);
    s.ev("session.step.ended", json!({ "assistantMessageID": m5, "finish": "stop", "cost": 0.14, "tokens": { "input": 8, "output": 980, "reasoning": 210, "cache": { "read": 41_600, "write": 1800 } } }));
    s.ev("session.execution.succeeded", json!({}));
    s.out.sort_by(|a, b| a.0.total_cmp(&b.0));
    s.out
}

const PLAN: &str = "## Plan

The palette lives in `tokens.ts` as one flat object, so dark mode is mostly a second set of values:

- add a `dark` palette beside `light`, chosen by a `data-theme` attribute on `<html>`;
- follow the system through `prefers-color-scheme` until you pick one;
- add an **Appearance** row to settings with *System*, *Light* and *Dark*.

```ts
export type Theme = \"system\" | \"light\" | \"dark\"

export function applyTheme(theme: Theme) {
  const dark = theme === \"dark\" ||
    (theme === \"system\" && matchMedia(\"(prefers-color-scheme: dark)\").matches)
  document.documentElement.dataset.theme = dark ? \"dark\" : \"light\"
}
```";

const SUMMARY: &str = "Done. Settings follow the system theme, and **Settings › Appearance** pins it to Light or Dark, saved on this device.

| File | Change |
| --- | --- |
| `settings/SettingsPage.tsx` | the Appearance row |
| `theme/applyTheme.ts` | picks the palette, follows the system |
| `theme/tokens.ts` | a dark palette beside the light one |
| `docs/CodeBlock.tsx` | syntax colors from the theme |

All 48 tests pass.";
