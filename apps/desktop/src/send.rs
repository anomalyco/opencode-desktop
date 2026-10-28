use super::*;

impl App {
    pub fn send(&mut self, tx: &mut Tx) {
        self.submit(false, tx);
    }

    /// Sends the composer's prompt. While the session is busy it steers, or queues when
    /// `alternate` (⌘⏎); see `queue.rs`.
    pub fn submit(&mut self, alternate: bool, tx: &mut Tx) {
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
            self.status = "Waiting for attachments to upload…".into();
            return;
        }
        self.close_mentions();
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
                tx.launch("prompt", format!("queued-{local}"));
            } else if !self.ends.at_end(&id) {
                tx.scroll_to_bottom_instant(format!("scroll-{id}"));
            }
            return;
        }
        let id = match self.route.clone() {
            Route::Session(id) => id,
            Route::Draft if self.live() => {
                if self.write(Write::Start { directory: self.draft_dir.clone(), prompt }) {
                    self.status = "Starting a session…".into();
                    self.composer.clear();
                    self.attach.files.clear();
                    self.quote.take();
                }
                return;
            }
            _ => {
                self.status = "Pick a session to send to".into();
                return;
            }
        };
        let local = match &self.source {
            Source::Live { .. } => {
                let local = prompt.id.clone().unwrap_or_default();
                if !self.write(Write::Send { session: id.clone(), prompt }) {
                    return;
                }
                self.attach.echoes.push((id.clone(), local.clone(), text.clone()));
                local
            }
            Source::Fixture => {
                let selection = self.selection();
                self.sends += 1;
                let reply = format!("msg_zzzzreply{:04}", self.sends);
                let at = self.clock;
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
                    self.playback.push_back((at + e["t"].as_f64().unwrap_or(0.0), e["type"].as_str().unwrap_or("").into(), d));
                }
                let sent = format!("msg_zzzzsent{:04}", self.sends);
                // A server admits every prompt to its inbox as a steer, then delivers it once
                // the run takes it up, as a live one does.
                let item = serde_json::json!({ "type": "user", "delivery": "steer", "payload": { "text": text, "metadata": { "displayText": wire.display } } });
                self.playback.push_back((at + 0.05, "session.inbox.enqueued".into(), serde_json::json!({ "sessionID": id, "inboxID": sent, "item": item })));
                self.playback.push_back((at + 0.7, "session.inbox.delivered".into(), serde_json::json!({ "sessionID": id, "inboxID": sent })));
                self.playback.make_contiguous().sort_by(|a, b| a.0.total_cmp(&b.0));
                self.wake_at_playback();
                sent
            }
        };
        opencode_kit::model::set_user_files(&local, std::mem::take(&mut self.attach.files));
        self.echo_mentions(&local);
        opencode_kit::model::set_user_quotes(&local, quotes);
        self.quote.take();
        self.land(&id, &local, tx);
        let t = self.transcripts.entry(id.clone()).or_insert_with(|| Transcript { session: id.clone(), messages: vec![] });
        t.messages.push(Message::User { id: local, text: wire.display });
        self.composer.clear();
    }

    /// Where sent prompt `local` shows up. When the timeline shows its end, it rises out of the
    /// composer, its words where they were typed. Read further up, nothing flies: the timeline
    /// cuts to its end, where the message already stands.
    fn land(&mut self, id: &str, local: &str, tx: &mut Tx) {
        if self.ends.at_end(id) {
            tx.launch("prompt", format!("u-{local}/bubble"));
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
            Source::Live { allow_send: false, .. } => self.status = "Read-only: relaunch without --read-only to act".into(),
            Source::Live { writes: Some(tx), .. } => return tx.send(w).is_ok(),
            _ => self.status = "Not connected to an OpenCode server".into(),
        }
        false
    }
}
