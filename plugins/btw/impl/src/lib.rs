//! Desktop's built-in BTW plugin (`packages/gui-plugins/src/btw`) as a native plugin: `/btw
//! <question>` asks the session's model a one-shot side question (`POST
//! /api/session/:id/generate`), without adding to the conversation, and shows it in a tab of its
//! own: the question as a prompt, Working while it runs, then the answer as a reply, or why it
//! failed with Retry. Each tab's chat is kept in the session's store until the tab closes.
//!
//! A built-in: this rlib declares it, `plugins/btw` exports it as a library, and
//! `opencode-desktop --features static-builtins` links it in.

mod machine;
mod panel;
mod store;

use opencode_plugin::prelude::*;
use opencode_plugin::system;

use machine::{ChatModel, Effect, Failure, Info, State, TabView};

/// The panel's id: tabs are `btw.main/<chat id>`, as Desktop's `btw:<id>`.
pub(crate) const PANEL: &str = "main";

const INSTRUCTIONS: &str = "The user is asking a quick side question about the conversation so far. Answer directly and concisely in markdown from what you already know. Do not call any tools and do not take any actions.";

pub struct Btw {
    state: State,
    /// Chats whose answer was copied, for the copied mark: (chat, "q" or "a").
    copied: Vec<(String, &'static str)>,
    /// Ids handed out by this instance, so two asks in one millisecond differ.
    counter: u64,
}

impl WindowPlugin for Btw {
    fn setup(cx: &mut WindowSetupCx<Self>) -> Result<Self, PluginError> {
        cx.add_command(
            CommandSpec::new("ask", "Ask a side question")
                .description("Get a one-shot answer without adding to the conversation")
                .section(CommandSection::Session)
                .slash("btw")
                .slash_args(true)
                .hidden(true)
                .needs_session(true),
            |s, cx| s.ask(cx),
        );
        cx.add_panel_instances(PanelSpec::new(PANEL, "/btw").icon("comment").transient(true), list, panel::render, panel::label, |s, cx| {
            let (id, session) = (cx.input().to_owned(), cx.input_extra().to_owned());
            let effects = s.state.removed(&session, &id);
            s.copied.retain(|(c, _)| *c != id);
            s.run(cx, &session, effects);
        });
        Ok(Btw { state: State::default(), copied: Vec::new(), counter: 0 })
    }

    /// A question's deadline came: it fails with Retry.
    fn poll(&mut self, cx: &mut PollCx) -> bool {
        let now = cx.now_ms();
        let due: Vec<(String, String, u64)> =
            self.state.running_attempts().filter(|(.., deadline)| *deadline <= now).map(|(s, id, a, _)| (s.to_owned(), id.to_owned(), a)).collect();
        let changed = !due.is_empty();
        for (session, id, attempt) in due {
            let effects = self.state.failed(&session, &id, attempt, Failure::Timeout);
            self.run(cx, &session, effects);
        }
        self.schedule(cx);
        changed
    }
}

/// The session's open chats this panel lists: one tab per question, titled by it.
fn list(s: &Btw, session: &str, open: &[String]) -> Vec<PanelTab> {
    let saved = store::load(session);
    open.iter()
        .filter_map(|id| match s.state.tab(&saved, session, id) {
            TabView::Ready(question) => Some(PanelTab { id: id.clone(), title: question, hidden: false }),
            TabView::Missing => None,
        })
        .collect()
}

/// The routed session's id, agent and model, as the host has them.
fn routed() -> Option<(String, String, Option<ChatModel>)> {
    let v: serde_json::Value = serde_json::from_str(&system::active_session()?).ok()?;
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    let model = (!s("modelID").is_empty()).then(|| ChatModel { provider: s("providerID"), id: s("modelID"), variant: s("variant") });
    Some((s("id"), s("agent"), model))
}

impl Btw {
    fn ask(&mut self, cx: &mut ActionCx) {
        let question = cx.input().trim().to_owned();
        if question.is_empty() {
            cx.toast("Add a question after /btw");
            return;
        }
        let Some((session, agent, model)) = routed().filter(|(id, ..)| !id.is_empty()) else { return };
        let now = cx.now_ms();
        self.counter += 1;
        let id = format!("{now:x}{:04x}", self.counter);
        let effects = self.state.asked(&session, &id, Info { question, created: Some(now), agent, model });
        self.run(cx, &session, effects);
    }

    pub(crate) fn retry(&mut self, cx: &mut ActionCx, session: &str, id: &str) {
        let saved = store::load(session);
        let (agent, model) = routed().map(|(_, a, m)| (a, m)).unwrap_or_default();
        let effects = self.state.retried(session, id, cx.now_ms(), saved.iter().find(|c| c.id == id), &agent, model);
        self.run(cx, session, effects);
    }

    fn run(&mut self, cx: &mut ActionCx, session: &str, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::PersistQuestion { id, info } => store::update(session, |chats| chats.push(machine::Saved { id, info, answer: None, duration: None })),
                Effect::OpenTab(id) => cx.open_panel_instance(PANEL, &id),
                Effect::PersistAnswer { id, answer, duration, agent, model } => store::update(session, |chats| {
                    if let Some(c) = chats.iter_mut().find(|c| c.id == id) {
                        c.answer = Some(answer.clone());
                        c.duration = Some(duration);
                        if !agent.is_empty() {
                            c.info.agent = agent.clone();
                        }
                        if model.is_some() {
                            c.info.model = model.clone();
                        }
                    }
                }),
                Effect::DeleteChat(id) => store::update(session, |chats| chats.retain(|c| c.id != id)),
                Effect::Generate { id, attempt, question } => self.generate(cx, session, &id, attempt, &question),
            }
        }
        self.schedule(cx);
    }

    fn generate(&mut self, cx: &mut ActionCx, session: &str, id: &str, attempt: u64, question: &str) {
        let body = serde_json::json!({ "prompt": format!("{INSTRUCTIONS}\n\n{question}") }).to_string();
        let (sid, cid) = (session.to_owned(), id.to_owned());
        let path = format!("/api/session/{session}/generate");
        cx.server_request("POST", &path, &body, move |s: &mut Btw, cx| {
            let effects = match cx.input_extra() {
                "" => {
                    let v: serde_json::Value = serde_json::from_str(cx.input()).unwrap_or_default();
                    let text = v["data"]["text"].as_str().or(v["text"].as_str()).unwrap_or_default().to_owned();
                    s.state.succeeded(&sid, &cid, attempt, &text, cx.now_ms())
                }
                error => {
                    let message = Some(error.trim().to_owned()).filter(|m| !m.is_empty());
                    s.state.failed(&sid, &cid, attempt, Failure::Error(message))
                }
            };
            s.run(cx, &sid, effects);
        });
    }

    /// Polls again when the next running question's deadline comes.
    fn schedule(&self, cx: &mut ActionCx) {
        let now = cx.now_ms();
        if let Some(next) = self.state.running_attempts().map(|(.., deadline)| deadline).min() {
            cx.wake_after((next - now).max(0) as u64);
        }
    }
}

opencode_plugin::declare_plugin!(window = Btw);
