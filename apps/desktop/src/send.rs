use super::*;

impl App {
    pub fn send(&mut self, tx: &mut Tx) {
        self.submit(false, tx);
    }

    /// Sends the composer's prompt. While the session is busy it steers, or queues when
    /// `alternate` (⌘⏎); see `queue.rs`.
    pub fn submit(&mut self, alternate: bool, tx: &mut Tx) {
        self.record_sent(|s| s.send_prompt(alternate, tx));
    }

    fn send_prompt(&mut self, alternate: bool, tx: &mut Tx) {
        if self.opening.is_some() { return; }
        if crate::dictation::submit(self, alternate) { return; }
        if self.add.shell {
            return self.submit_shell(alternate, tx);
        }
        let wire = self.mention_wire();
        if wire.text.is_empty() && !self.attach.any() && !self.quote.any() {
            return;
        }
        if !self.attach.uploading.is_empty() {
            crate::status_toast::say(self, "Attachments are still uploading", "Send again once they finish.");
            return;
        }
        self.close_mentions();
        if self.run_slash(&wire.text, tx) {
            return;
        }
        let command = self.slash_command(&wire.text);
        // Commands take no path attachments or quotes; they wait in the composer for the next prompt.
        let refs = if command.is_none() { std::mem::take(&mut self.attach.paths) } else { Vec::new() };
        let quotes = if command.is_none() { self.quote.quotes() } else { Vec::new() };
        let text = match &command {
            Some((_, args)) => args.clone(),
            None => {
                let quoted = quotes.iter().map(opencode_kit::model::Quote::prompt_text);
                std::iter::once(attachments::path_lines(&wire.text, &refs)).filter(|t| !t.is_empty()).chain(quoted).collect::<Vec<_>>().join("\n")
            }
        };
        // A prompt of only quotes leaves from its first quote's chip; any other from its words.
        let from = match self.quote.items.first() {
            Some(q) if wire.display.trim().is_empty() => format!("{}/card/title", crate::quote_cards::chip_key(q.key)),
            _ => "prompt".to_owned(),
        };
        let (agent, model) = self.picked_choice();
        // The server's user message takes this id, so it replaces the local copy below.
        let prompt = Prompt {
            id: Some(opencode_kit::client::message_id()),
            text: text.clone(),
            files: wire.files.into_iter().chain(attachments::inputs(&self.attach.files)).collect(),
            agents: wire.agents,
            skills: wire.skills,
            display_text: Some(wire.display.clone()),
            delivery: None,
            attachments: refs,
            command: command.map(|(name, _)| name),
            quotes,
            agent,
            model,
        };
        if prompt.command.is_some() {
            return self.send_command(prompt, alternate, tx);
        }
        let quotes = prompt.quotes.clone();
        if let Route::Session(id) = self.route.clone()
            && self.busy(&id)
        {
            let local = prompt.id.clone().unwrap_or_default();
            self.echo_mentions(&local);
            self.admit(&id, prompt, if alternate { Delivery::Queue } else { Delivery::Steer });
            if alternate {
                if crate::view::shows_whole_prompt() {
                    tx.launch("prompt", format!("queued-{local}"));
                }
            } else {
                self.land(&id, &local, &from, tx);
            }
            return;
        }
        // From the new-session page, the session shows at once under an id chosen here.
        let starting = self.route == Route::Draft;
        let id = match self.route.clone() {
            Route::Session(id) => id,
            Route::Draft if self.live() && crate::new_session::before() => {
                if self.write(Write::Start { directory: self.draft_dir.clone(), prompt }) {
                    self.composer.clear();
                    self.attach.files.clear();
                    self.quote.take();
                }
                return;
            }
            Route::Draft => opencode_kit::client::session_id(),
            _ => {
                crate::status_toast::say(self, "Open a session to send to", "");
                return;
            }
        };
        let local = match &self.source {
            Source::Live { .. } => {
                let local = prompt.id.clone().unwrap_or_default();
                let sent = crate::send_failed::Sent::new(&id, &local, crate::prompt_history::Entry::of(&self.composer), self.attach.files.clone(), prompt.attachments.clone(), false);
                let staged = self.session(&id).and_then(|i| i.revert.as_ref()).is_some();
                let w = if starting {
                    Write::StartAs { session: id.clone(), directory: self.draft_dir.clone(), prompt }
                } else if staged {
                    Write::SendFromRevert { session: id.clone(), prompt }
                } else {
                    self.send_write(&id, prompt)
                };
                if !self.write(w) {
                    return;
                }
                if starting {
                    self.start(&id, tx);
                }
                if staged {
                    self.commit_staged_revert(&id);
                }
                self.attach.echoes.push((id.clone(), local.clone(), text.clone()));
                self.unsent.note(sent);
                local
            }
            Source::Fixture => {
                if starting {
                    self.start(&id, tx);
                }
                self.commit_staged_revert(&id);
                let selection = self.selection();
                self.sends += 1;
                let reply = format!("msg_zzzzreply{:04}", self.sends);
                let at = self.clock;
                self.serve_choice(&id, &prompt, at);
                for e in self.fixture["reply"].as_array().cloned().unwrap_or_default() {
                    let mut d = e["data"].clone();
                    d["sessionID"] = id.clone().into();
                    d["assistantMessageID"] = reply.clone().into();
                    if let Some(call) = d["id"].as_str() {
                        d["id"] = format!("{call}-{:04}", self.sends).into();
                    }
                    if e["type"] == "session.step.started" {
                        d["agent"] = selection.agent.clone().into();
                        d["model"] = serde_json::json!({"id": selection.model.id, "providerID": selection.model.provider, "variant": selection.model.variant});
                    }
                    self.schedule(at + e["t"].as_f64().unwrap_or(0.0), e["type"].as_str().unwrap_or(""), d);
                }
                let sent = format!("msg_zzzzsent{:04}", self.sends);
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
        opencode_kit::model::set_user_files(&local, std::mem::take(&mut self.attach.files));
        self.echo_mentions(&local);
        opencode_kit::model::set_user_quotes(&local, quotes);
        self.quote.take();
        self.land(&id, &local, &from, tx);
        let t = self.transcripts.entry(id.clone()).or_insert_with(|| Transcript { session: id.clone(), messages: vec![] });
        t.messages.push(Message::User { id: local, text: wire.display });
        self.composer.clear();
    }

    /// Where sent prompt `local` shows up. When the timeline shows its end, it rises out of the
    /// composer, its words where they were typed. Read further up, nothing flies: the timeline
    /// cuts to its end, where the message already stands. A prompt longer than the composer,
    /// whose words start out of sight in it, arrives where it lands instead of flying.
    fn land(&mut self, id: &str, local: &str, from: &str, tx: &mut Tx) {
        if self.ends.at_end(id) {
            if crate::view::shows_whole_prompt() {
                tx.launch(from, format!("u-{local}/bubble"));
            }
        } else {
            tx.scroll_to_bottom_instant(format!("scroll-{id}"));
        }
    }

    /// Queues a write to the server; false (with a status line) when this build can't write.
    pub fn write(&mut self, w: Write) -> bool {
        if self.stage.is_some() {
            // A story's answers go nowhere; it acts as if they were sent.
            self.status = "Storybook: not sent".into();
            return true;
        }
        match &self.source {
            Source::Live { allow_send: false, .. } => crate::status_toast::fail(self, "Nothing was sent", "This window is read-only. Relaunch without --read-only to act."),
            Source::Live { writes: Some(tx), .. } => return tx.send(w).is_ok(),
            _ => crate::status_toast::fail(self, "Not connected to an OpenCode server", "Nothing was sent."),
        }
        false
    }
}
