use opencode_kit::client::{Prompt, Write};
use opencode_kit::model::{Delivery, Message, Transcript};
use rune::{TextEdit, Tx};

use crate::pickers::PickerHost as _;
use crate::{App, Route, Source, attachments};

/// How the user asked to submit the composer (`editor.tsx:388-394`, `#63109`, `#63111`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmitMode {
    #[default]
    Default,
    /// `Mod+Enter` or the alternate-delivery toolbar hint: flips Steer/Queue, or steers the first
    /// queued prompt immediately when the normal-mode composer is empty (`#63111`).
    Alternate,
    /// `Alt+Enter`: queues the prompt at the front of the queue while the session is busy, without
    /// steering, regardless of the follow-up preference (`#63109`).
    QueueFirst,
    /// `Ctrl+Enter` on macOS (`Alt+Shift+Enter` elsewhere), or `Shift+Click` on Send: submits
    /// with default delivery without scrolling a scrolled-up timeline back to the bottom.
    InPlace,
}

impl App {
    pub fn send(&mut self, tx: &mut Tx) {
        self.submit_mode(SubmitMode::Default, tx);
    }

    /// Sends the composer's prompt. While the session is busy it follows the follow-up preference,
    /// or the opposite delivery when `alternate` (`⌘⏎`); see `queue.rs`.
    pub fn submit(&mut self, alternate: bool, tx: &mut Tx) {
        self.submit_mode(if alternate { SubmitMode::Alternate } else { SubmitMode::Default }, tx);
    }

    /// Sends the composer's prompt with explicit [`SubmitMode`] (`Default`, `Alternate`, or
    /// `QueueFirst`).
    pub fn submit_mode(&mut self, mode: SubmitMode, tx: &mut Tx) {
        let alternate = mode == SubmitMode::Alternate;
        // An undo of a queued prompt holds the submit; a queued prompt open in the composer goes
        // back to the queue (Return / Alt+Return) or steers (Mod+Return) instead of a new send
        // (`composer/model.ts:540-560`).
        if let Route::Session { id, .. } = &*self.route
            && let Some(q) = self.kernel.slots.get(&crate::queue::QUEUE_OPS).get(id)
        {
            if q.undoing() {
                return;
            }
            if q.editing().is_some() {
                tx.instant();
                return self.confirm_queue_edit(alternate);
            }
        }
        // With no draft to send, Mod+Enter in a normal-mode composer promotes and steers the next
        // queued prompt immediately (`composer/model.ts:562-568`, `#63111`). Plain Enter, Alt+Enter,
        // parked sessions, and shell mode leave the queue alone.
        if mode == SubmitMode::Alternate
            && !self.kernel.slots.get(&crate::add_menu::ADD_MENU).shell()
            && !crate::dictation::is_active(self)
            && self.composer.text.trim().is_empty()
            && !self.composer_ext.attach.any()
            && self.composer_ext.attach.context.is_empty()
            && self.composer_ext.attach.uploads.is_empty()
            && !self.kernel.slots.get(&crate::quote::QUOTE).any()
        {
            if self.opening.is_none()
                && !self.dock_shown()
                && let Route::Session { id, .. } = (*self.route).clone()
                && self.busy(&id)
            {
                tx.instant();
                self.steer_first_queued(&id);
            }
            return;
        }
        let typed = !self.composer.text.is_empty();
        rune::marks::mark("send.submit");
        self.record_sent(|s| s.send_prompt(mode, tx));
        rune::marks::mark("send.handled");
        // A narrow window lets go of the composer once it sends (`submit.ts:436`).
        if self.narrow() && typed && self.composer.text.is_empty() {
            self.composer.focused = false;
        }
    }

    fn send_prompt(&mut self, mode: SubmitMode, tx: &mut Tx) {
        let alternate = mode == SubmitMode::Alternate;
        // Held, not dropped, while the session still opens (`submit.rs`).
        if self.hold_submit(mode) { return; }
        if crate::dictation::submit_mode(self, mode) { return; }
        if self.kernel.slots.get(&crate::add_menu::ADD_MENU).shell() {
            return self.submit_shell(alternate, tx);
        }
        if matches!(mode, SubmitMode::Default | SubmitMode::InPlace) && self.retry_restored() { return; }
        let wire = self.mention_wire();
        if wire.text.is_empty() && !self.composer_ext.attach.any() && !self.kernel.slots.get(&crate::quote::QUOTE).any() {
            return;
        }
        if !self.composer_ext.attach.uploads.is_empty() {
            crate::status_toast::say(self, opencode_i18n::t!("rune.send.uploading.title"), opencode_i18n::t!("rune.send.uploading.description"));
            return;
        }
        self.suggest(crate::suggestions::machine::Event::Close, tx);
        if self.run_slash(&wire.text, tx) {
            return;
        }
        let command = self.slash_command(&wire.text);
        // Commands take no path attachments or quotes; they wait in the composer for the next prompt.
        let refs = if command.is_none() { std::mem::take(&mut self.composer_ext.attach.paths) } else { Vec::new() };
        let quotes = if command.is_none() { self.kernel.slots.get(&crate::quote::QUOTE).quotes() } else { Vec::new() };
        // A file comment attaches its lines (`request.ts:186-207`), before the file context.
        let directory = self.composer_directory();
        let commented = quotes.iter().filter_map(|c| c.file()).map(|f| {
            let (a, b) = f.lines.unwrap_or((0, 0));
            crate::mentions::context_input(&directory, &f.path, a, b)
        });
        let context: Vec<_> = if command.is_none() { commented.chain(crate::add_selection::take(self)).collect() } else { Vec::new() };
        let text = match &command {
            Some((_, args)) => args.clone(),
            None => {
                let quoted = quotes.iter().map(opencode_kit::model::PromptComment::prompt_text);
                std::iter::once(attachments::path_lines(&wire.text, &refs)).filter(|t| !t.is_empty()).chain(quoted).collect::<Vec<_>>().join("\n")
            }
        };
        // A prompt of only quotes leaves from its first quote's chip; any other from its words.
        let from = match self.kernel.slots.get(&crate::quote::QUOTE).items.first() {
            Some(q) if wire.display.trim().is_empty() => format!("{}/card/title", crate::quote_cards::chip_key(q.key)),
            _ => "prompt".to_owned(),
        };
        let (agent, model) = self.picked_choice();
        // The server's user message takes this id, so it replaces the local copy below.
        let prompt = Prompt {
            id: Some(opencode_kit::client::message_id()),
            text: text.clone(),
            files: wire.files.into_iter().chain(context).chain(attachments::inputs(&self.composer_ext.attach.files)).collect(),
            agents: wire.agents,
            skills: wire.skills,
            display_text: Some(wire.display.clone()),
            delivery: None,
            attachments: refs,
            command: command.map(|(name, _)| name),
            comments: quotes,
            agent,
            model,
        };
        if prompt.command.is_some() {
            return self.send_command(prompt, alternate, tx);
        }
        let quotes = prompt.comments.clone();
        if let Route::Session { id, .. } = (*self.route).clone()
            && (self.busy(&id) || self.steers_waiting(&id))
        {
            let local = prompt.id.clone().unwrap_or_default();
            // Alt+Enter always queues first while busy (`#63109`); Enter does what Settings ›
            // Follow-up behavior says and ⌘⏎ the other; with no run to queue behind, both send
            // as a steer does.
            let queue_first = mode == SubmitMode::QueueFirst && self.busy(&id);
            let queue = queue_first || ((alternate != (self.settings.follow_up == crate::settings::FollowUp::Queue)) && self.busy(&id));
            if !queue {
                self.note_pick(&local, &prompt);
            }
            self.echo_mentions(&local);
            opencode_kit::mention::set_user_path_attachments(&local, prompt.attachments.clone());
            self.admit_with(&id, prompt, if queue { Delivery::Queue } else { Delivery::Steer }, queue_first);
            if queue {
                if crate::view::shows_whole_prompt() {
                    tx.launch("prompt", format!("queued-{local}"));
                }
            } else if mode == SubmitMode::InPlace {
                session_view::composer::land_in_place(self, &id, &local, &from, tx);
            } else {
                session_view::composer::land(self, &id, &local, &from, tx);
            }
            return;
        }
        // From the new-session page, the session shows at once under an id chosen here.
        let starting = matches!(*self.route, Route::Draft(_));
        let id = match (*self.route).clone() {
            Route::Session { id, .. } => id,
            Route::Draft(_) => opencode_kit::client::session_id(),
            _ => {
                crate::status_toast::say(self, opencode_i18n::t!("rune.composer.openSession"), "");
                return;
            }
        };
        // The local copy shows the files it names by path as cards, as the server's will.
        let refs = prompt.attachments.clone();
        let local = match &*self.source {
            Source::Live { .. } => {
                let local = prompt.id.clone().unwrap_or_default();
                // It runs now, so the server applies its switch before it has the prompt.
                self.note_pick(&local, &prompt);
                let sent = crate::send_failed::Sent::new(&id, &local, crate::prompt_history::Entry::of(&self.composer), self.composer_ext.attach.files.clone(), prompt.attachments.clone(), false).with_prompt(&prompt);
                let staged = self.session(&id).and_then(|i| i.revert.as_ref()).is_some();
                let journal = matches!(*self.source, Source::Live {allow_send:true,..}) && self.fixture_run.stage.is_none();
                if starting && journal {
                    // The start runs its phases and hands the prompt to the outbox (`new_session`).
                    let paths=sent.paths.clone();
                    if !self.begin_start(&id, sent, prompt, tx) {self.composer_ext.attach.paths.extend(paths);return;}
                } else {
                    let w = if starting {
                        self.start_write(id.clone(), prompt)
                    } else if staged {
                        Write::SendFromRevert { session: id.clone(), prompt }
                    } else {
                        self.send_write(&id, prompt)
                    };
                    if journal {
                        let paths=sent.paths.clone();
                        rune::marks::mark("send.enqueue");
                        if !self.enqueue_prompt(sent,w) {self.composer_ext.attach.paths.extend(paths);return;}
                        rune::marks::mark("send.enqueued");
                    } else {
                        if !self.write(w) {self.composer_ext.attach.paths.extend(sent.paths);return;}
                        self.unsent.note(sent);
                        self.composer_ext.attach.echoes.push((id.clone(), local.clone(), text.clone()));
                    }
                    if starting {
                        self.start(&id, tx);
                    }
                }
                if staged {
                    self.commit_staged_revert(&id);
                }
                local
            }
            Source::Fixture => {
                if starting {
                    self.start(&id, tx);
                }
                self.commit_staged_revert(&id);
                let selection = self.selection();
                *self.queue.sends += 1;
                let reply = format!("msg_zzzzreply{:04}", *self.queue.sends);
                let at = *self.clock;
                self.serve_choice(&id, &prompt, at);
                for e in self.fixture_run.fixture["reply"].as_array().cloned().unwrap_or_default() {
                    let mut d = e["data"].clone();
                    d["sessionID"] = id.clone().into();
                    d["assistantMessageID"] = reply.clone().into();
                    if let Some(call) = d["id"].as_str() {
                        d["id"] = format!("{call}-{:04}", *self.queue.sends).into();
                    }
                    if e["type"] == "session.step.started" {
                        d["agent"] = selection.agent.clone().into();
                        d["model"] = serde_json::json!({"id": selection.model.id, "providerID": selection.model.provider, "variant": selection.model.variant});
                    }
                    self.schedule(at + e["t"].as_f64().unwrap_or(0.0), e["type"].as_str().unwrap_or(""), d);
                }
                let sent = format!("msg_zzzzsent{:04}", *self.queue.sends);
                // A server admits every prompt to its inbox as a steer, then delivers it once
                // the run takes it up, as a live one does.
                let item = serde_json::json!({ "type": "user", "delivery": "steer", "payload": { "text": text, "metadata": { "displayText": wire.display } } });
                self.schedule(at + 0.05, "session.inbox.enqueued", serde_json::json!({ "sessionID": id, "inboxID": sent, "item": item }));
                self.schedule(at + 0.7, "session.inbox.delivered", serde_json::json!({ "sessionID": id, "inboxID": sent }));
                self.wake_at_playback();
                sent
            }
        };
        // A prompt sent while a stop settles runs next, so its Working shows.
        self.stops.forget(&id);
        opencode_kit::model::set_user_files(&local, std::mem::take(&mut self.composer_ext.attach.files));
        opencode_kit::mention::set_user_path_attachments(&local, refs);
        self.echo_wire_mentions(&local, wire.marks);
        opencode_kit::model::set_user_comments(&local, quotes);
        self.kernel.slots.get_mut(&crate::quote::QUOTE).take();
        // Like the TUI, sent comments go with the prompt (`submit.ts:413-417`).
        self.clear_line_comments(&id);
        if mode == SubmitMode::InPlace {
            session_view::composer::land_in_place(self, &id, &local, &from, tx);
        } else {
            session_view::composer::land(self, &id, &local, &from, tx);
        }
        let t = self.transcripts.entry(id.clone()).or_insert_with(|| Transcript { session: id.clone(), messages: vec![] });
        // As the server's copy will read, so a session mention's badge stays put when it lands.
        t.messages.push(Message::User { id: local, text: wire.text });
        self.composer.clear();
        rune::marks::mark("send.landed");
    }

    /// Desktop's `quote.send` (`composer/machine.ts` `quote`, `submit.ts:144-166`): a prompt of
    /// quote `key` alone, with its comment, in the delivery Return doesn't use.
    pub(crate) fn send_quote(&mut self, key: u64, tx: &mut Tx) -> Option<Option<String>> {
        self.send_comment(key, true, tx)
    }

    /// Desktop's `comment.send` (`submit.ts:383-409`, `UP-137` `#65264`): sends one attached
    /// quote, browser note, or file/review line comment as its own prompt without disturbing the
    /// composer draft, other attachments, or other file/review line comments. Uses
    /// `SubmitMode::Alternate` when `alternate` is true (timeline/terminal quotes) and
    /// `SubmitMode::Default` when false (browser, file, review comments — Enter's delivery).
    pub(crate) fn send_comment(&mut self, key: u64, alternate: bool, tx: &mut Tx) -> Option<Option<String>> {
        let quote = self.kernel.slots.get(&crate::quote::QUOTE).items.iter().find(|q| q.key == key)?.clone();
        let before = self.newest_send(None);
        // What the composer holds steps aside while the comment goes on its own.
        let composer = std::mem::replace(&mut *self.composer, TextEdit::new(""));
        let saved_lc = self.active_session_id().map(str::to_owned).and_then(|sid| {
            let list = self.kernel.slots.get(&crate::line_comments::LINE_COMMENTS).sessions.get(&sid)?.list.clone();
            Some((sid, list))
        });
        let q = self.kernel.slots.get_mut(&crate::quote::QUOTE);
        let others: Vec<_> = q.items.iter().filter(|q| q.key != key).cloned().collect();
        q.items = vec![quote.clone()];
        let comment = std::mem::take(&mut q.comment);
        let files = std::mem::take(&mut self.composer_ext.attach.files);
        let paths = std::mem::take(&mut self.composer_ext.attach.paths);
        let context = std::mem::take(&mut self.composer_ext.attach.context);
        let mode = if alternate { SubmitMode::Alternate } else { SubmitMode::Default };
        self.send_prompt(mode, tx);
        if let Some((sid, list)) = saved_lc
            && let Some(session) = self.kernel.slots.get_mut(&crate::line_comments::LINE_COMMENTS).sessions.get_mut(&sid)
        {
            session.list = list;
        }
        let q = self.kernel.slots.get_mut(&crate::quote::QUOTE);
        let went = q.items.is_empty();
        q.comment = comment;
        q.items = others;
        // In its place, until its send settles.
        let at = q.items.iter().position(|q| q.key > key).unwrap_or(q.items.len());
        q.items.insert(at, quote);
        *self.composer = composer;
        self.composer_ext.attach.files = files;
        self.composer_ext.attach.paths = paths;
        self.composer_ext.attach.context = context;
        went.then(|| self.newest_send(before.as_deref()))
    }

    /// Steers still wait after their run stopped: the server delivers them before any later
    /// prompt, so one sent now waits behind them rather than landing above them. A staged
    /// revert drops them, so a send from it goes straight in.
    fn steers_waiting(&self, id: &str) -> bool {
        self.session(id).and_then(|i| i.revert.as_ref()).is_none() && self.inbox_for(id).is_some_and(|i| i.steering().next().is_some())
    }

    /// Queues a write to the server; false (with a status line) when this build can't write.
    pub fn write(&mut self, w: Write) -> bool {
        if self.fixture_run.stage.is_some() {
            // A story's answers go nowhere; it acts as if they were sent.
            *self.diag.status = "Storybook: not sent".into();
            return true;
        }
        match &*self.source {
            Source::Live { allow_send: false, .. } => crate::status_toast::fail(self, opencode_i18n::t!("rune.send.nothingSent"), opencode_i18n::t!("rune.jobs.readOnly")),
            Source::Live { writes: Some(tx), .. } => {
                // What a write asks the server for, the window has shown already: the server's
                // other windows draw it too.
                let sent = tx.send(w).is_ok();
                crate::windows::peer::touch(self);
                return sent;
            }
            _ => crate::status_toast::fail(self, opencode_i18n::t!("rune.send.disconnected"), opencode_i18n::t!("rune.send.nothingSentSentence")),
        }
        false
    }
}
