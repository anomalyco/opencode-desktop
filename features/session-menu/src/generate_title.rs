//! Desktop's `title.set` (`packages/app/src/session/timeline/controller.tsx:455-494`,
//! opencode#66396): `/rename <title>` renames the session, and a bare `/rename`, or Generate
//! session title, asks the server to generate a new title from the conversation so far. Both
//! are the session's update; an empty title is the ask to generate.
//!
//! Generating takes a model call, so a session has one ask out at a time: a repeat does nothing
//! until it is answered, and its title shimmers in the header meanwhile.

use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;
use opencode_kit::wire_error::WireError;

use crate::SessionMenuHost;

/// The sessions whose generated title the server has been asked for and has not answered: a
/// session is either among them or not.
#[derive(Default)]
pub struct Generating(Vec<(ServerKey, String)>);

pub enum Event<'a> {
    /// A generated title is wanted for `session` on `server`.
    Asked { server: &'a ServerKey, session: &'a str },
    /// The server answered the ask, or it could not be sent.
    Replied { server: &'a ServerKey, session: &'a str },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Send the session's update with an empty title.
    Send,
}

impl Generating {
    /// Whether `session` on `server` waits for its generated title.
    pub fn has(&self, server: &ServerKey, session: &str) -> bool {
        self.0.iter().any(|(s, id)| s == server && id == session)
    }

    pub fn step(&mut self, event: Event<'_>) -> Option<Effect> {
        match event {
            // A repeat while one is in flight would only start a second model call.
            Event::Asked { server, session } if self.has(server, session) => None,
            Event::Asked { server, session } => {
                self.0.push((server.clone(), session.to_owned()));
                Some(Effect::Send)
            }
            Event::Replied { server, session } => {
                self.0.retain(|(s, id)| !(s == server && id == session));
                None
            }
        }
    }
}

/// `/rename` with `words` after it, or with none: the open session `id` takes them as its
/// title, or the server is asked to generate one.
pub fn set_title(host: &mut dyn SessionMenuHost, id: &str, words: &str) {
    let words = words.trim();
    if !words.is_empty() {
        // Sent, and shown when the server's own event names the session again
        // (`controller.tsx:472-478`): nothing changes here before that, so a rename the server
        // refuses, or one that could not be sent, leaves the title as it is.
        host.write_server(Write::Rename { session: id.to_owned(), title: words.to_owned() });
        return;
    }
    let server = host.active_server();
    if host.menus_mut().generating.step(Event::Asked { server: &server, session: id }).is_none() {
        return;
    }
    // A story's write goes nowhere and a build that can't write sends none (and says so): each
    // answers at once.
    let sent = host.write_server(Write::Rename { session: id.to_owned(), title: String::new() });
    if host.stage_active() || !sent {
        host.menus_mut().generating.step(Event::Replied { server: &server, session: id });
    }
}

/// The server answered the ask for `session`'s generated title (`client.session.write`, a
/// rename with an empty title).
pub fn replied(host: &mut dyn SessionMenuHost, session: &str, error: Option<WireError>) {
    // The reply comes in on its own server, checked out for it.
    let server = host.active_server();
    host.menus_mut().generating.step(Event::Replied { server: &server, session });
    match error {
        // `controller.tsx:480-482`.
        Some(e) => host.toast_wire_error_titled(opencode_i18n::t!("common.requestFailed"), e),
        // The generated title arrives with the session's event; the session is read again in
        // case the answer beat it (`controller.tsx:474-478`).
        None => host.refresh_session(session),
    }
}
