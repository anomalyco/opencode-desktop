//! Rewind (Settings › Developer, on by default): anything that just moved wrong. While it's on, Rune
//! records the last minute of the window's frames (`rune::rewind`): each keyed element's rect and
//! opacity, never pixels. ⌘⇧⌥R freezes the window and lays the scrubber over it, with what moved
//! in each frame outlined and the jumps marked on its track. Send writes the selected stretch as
//! a folder (summary, motion trace, contact sheet) in the app's data folder and sends it to the
//! open session as a message.

use std::path::PathBuf;

use opencode_kit::model::Message;
use opencode_kit::prompt::{PathAttachment, Prompt};
use rune::{Key, KeyEvent};
use rune::rewind::{Keyed, Rewind, Scrub, Sent};
use rune::{El, Tx};

use crate::palette::Pal;
use opencode_kit::model::Transcript;

use crate::{App, Route, Source};

#[cfg(test)]
mod tests;
mod story;

pub struct Glitch {
    pub rewind: Rewind,
    pub scrub: Scrub,
    /// The recorder is attached to the window.
    attached: bool,
    /// Where the Dev › Rewind story's card sits (`story.rs`).
    pub demo_y: f32,
}

impl Default for Glitch {
    fn default() -> Self {
        Glitch { rewind: Rewind::new(), scrub: Scrub::default(), attached: false, demo_y: 0.0 }
    }
}

/// Attaches the recorder while the setting is on, and lets it go when it's turned off.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let want = s.settings.record_glitches;
    if want == s.glitch.attached {
        return false;
    }
    s.glitch.attached = want;
    tx.record_rewind(want.then_some(&s.glitch.rewind));
    // Attaching shows nothing new; only closing an open scrubber needs a frame.
    let open = s.glitch.scrub.is_open();
    if !want {
        s.glitch.scrub.close();
    }
    !want && open
}

fn is_chord(ev: &KeyEvent) -> bool {
    matches!(ev.key, Key::Command(c) if c.eq_ignore_ascii_case(&'r')) && ev.mods.shift && ev.mods.alt
}

/// ⌘⇧⌥R freezes the window (and closes the scrubber again); while it's open, it takes every key.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.glitch.scrub.is_open() {
        if is_chord(ev) {
            s.glitch.scrub.close();
            return true;
        }
        if let Keyed::Send(sent) = s.glitch.scrub.key(ev) {
            send(s, sent, tx);
        }
        return true;
    }
    if !is_chord(ev) {
        return false;
    }
    if !s.glitch.attached {
        crate::status_toast::say(s, "Rewind is off", "Turn on Rewind in Settings › Developer.");
        return true;
    }
    freeze(s);
    true
}

/// Stops recording and opens the scrubber on the last frame.
pub fn freeze(s: &mut App) {
    s.composer.focused = false;
    s.search.focused = false;
    let rewind = s.glitch.rewind.clone();
    s.glitch.scrub.open(&rewind);
}

/// The app's view, paused while the scrubber shows, with the scrubber over it. The app is wrapped
/// the same way either way, so nothing in it changes path when the scrubber opens.
pub fn wrap(s: &App, app: El) -> El {
    rune::el()
        .w_full()
        .h_full()
        .center()
        .child(app.time_scale(if s.glitch.scrub.is_open() { 0.0 } else { 1.0 }))
        .child(s.glitch.scrub.view(|s: &mut App| &mut s.glitch.scrub, send))
}

/// Where a bundle goes: `<data dir>/rune-opencode/glitches/<ms>`, or the temp folder in tests
/// and headless runs.
fn bundle_dir(s: &App) -> PathBuf {
    let base = if s.persists() { crate::restore::data_dir() } else { None };
    base.unwrap_or_else(|| std::env::temp_dir().join("rune-opencode")).join("glitches").join(crate::wall_ms().to_string())
}

/// The session the bundle goes to: the open one, or the last tab from Home.
fn target(s: &App) -> Option<String> {
    match &s.route {
        Route::Session(id) => Some(id.clone()),
        _ => s.tabs.last().map(|t| t.id.clone()),
    }
}

/// The message a bundle sends: its summary to read, with the trace and contact sheet attached
/// by path.
pub fn prompt(bundle: &rune::rewind::Bundle) -> Prompt {
    let [_, trace, contact] = bundle.files();
    let path = |p: PathBuf| p.to_string_lossy().into_owned();
    let attachments = vec![
        PathAttachment { name: "contact.png".into(), mime: "image/png".into(), path: path(contact) },
        PathAttachment { name: "trace.jsonl".into(), mime: "application/x-ndjson".into(), path: path(trace) },
    ];
    let summary = bundle.summary().trim_end().to_owned();
    Prompt {
        id: Some(opencode_kit::client::message_id()),
        text: crate::attachments::path_lines(&summary, &attachments),
        display_text: Some(summary),
        attachments,
        ..Default::default()
    }
}

fn send(s: &mut App, sent: Sent, _: &mut Tx) {
    let Some(session) = target(s) else {
        crate::status_toast::say(s, "Open a session to send to", "The rewind stays open until you send it.");
        return;
    };
    let bundle = match sent.write(&bundle_dir(s)) {
        Ok(b) => b,
        Err(e) => return crate::status_toast::fail(s, "Couldn't save the rewind", e.to_string()),
    };
    let prompt = prompt(&bundle);
    let local = prompt.id.clone().unwrap_or_default();
    let display = prompt.display_text.clone().unwrap_or_default();
    if matches!(s.source, Source::Live { .. }) {
        let busy = s.busy(&session);
        let w = s.send_write(&session, prompt);
        if !s.write(w) {
            return;
        }
        if busy {
            // The server's inbox echo shows it where it waits.
            return close(s, &session);
        }
    }
    let t = s.transcripts.entry(session.clone()).or_insert_with(|| Transcript { session: session.clone(), messages: vec![] });
    t.messages.push(Message::User { id: local, text: display });
    close(s, &session);
}

fn close(s: &mut App, session: &str) {
    s.glitch.scrub.close();
    let title = s.sessions.iter().find(|i| i.id == session).map(|i| i.title.clone()).filter(|t| !t.is_empty());
    s.status = format!("Sent to {}", title.as_deref().unwrap_or("the session"));
}

/// Settings › Developer.
pub fn settings_section(s: &App, p: &Pal) -> El {
    let on = s.settings.record_glitches;
    crate::settings::section(p, "Developer", crate::settings::settings_card(p, "developer-list", [(
        "record-glitches",
        "Rewind",
        "Keep the last minute of the window. ⌘⇧⌥R freezes it so you can step back to what you didn't like and send it to the session.",
        crate::settings::switch(p, "record-glitches-switch", on).on_press(|s: &mut App| s.change_settings(|st| st.record_glitches = !st.record_glitches)),
    )]))
}
