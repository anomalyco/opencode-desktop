//! Reverts another client stages, clears or commits (`session.revert.*`).

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};
    use serde_json::json;

    use crate::App;

    const SID: &str = "ses_rv";

    fn fixture(revert: Option<&str>) -> String {
        let mut session = json!({ "id": SID, "projectID": "p", "title": "Revert", "agent": "build", "time": { "created": 1, "updated": 2 } });
        if let Some(id) = revert {
            session["revert"] = json!({ "messageID": id });
        }
        json!({
            "now": 10,
            "open": SID,
            "projects": [{ "id": "p", "worktree": "/w", "time": { "created": 1 } }],
            "sessions": [session],
            "messages": { SID: [
                { "id": "msg_1", "type": "user", "text": "keep me", "time": { "created": 1 } },
                { "id": "msg_2", "type": "assistant", "content": [{ "type": "text", "text": "kept answer" }], "time": { "created": 2, "completed": 3 } },
                { "id": "msg_3", "type": "user", "text": "take me back", "time": { "created": 4 } },
                { "id": "msg_4", "type": "assistant", "content": [{ "type": "text", "text": "doomed answer" }], "error": { "message": "Step interrupted" }, "time": { "created": 5, "completed": 6 } }
            ] }
        })
        .to_string()
    }

    fn app(revert: Option<&str>) -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(&fixture(revert));
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        rt
    }

    fn shows(rt: &Runtime<App>, s: &str) -> bool {
        rt.elements().into_iter().any(|e| !e.exiting && e.text.as_deref().is_some_and(|t| t.contains(s)))
    }

    fn event(rt: &mut Runtime<App>, ty: &str, data: serde_json::Value) {
        rt.update(|s, _| {
            s.apply_event(ty, &data);
        });
        rt.frame(1.0 / 120.0);
    }

    #[test]
    fn staged_hides_from_the_boundary_and_cleared_shows_it_again() {
        let mut rt = app(None);
        assert!(shows(&rt, "take me back") && shows(&rt, "doomed answer") && shows(&rt, "Step interrupted"));

        event(&mut rt, "session.revert.staged", json!({ "sessionID": SID, "revert": { "messageID": "msg_3" } }));
        assert!(shows(&rt, "keep me") && shows(&rt, "kept answer"));
        assert!(!shows(&rt, "take me back") && !shows(&rt, "doomed answer") && !shows(&rt, "Step interrupted"));
        assert_eq!(rt.state.transcripts[SID].messages.len(), 4, "staged keeps the rows for a redo");

        event(&mut rt, "session.revert.cleared", json!({ "sessionID": SID }));
        assert!(shows(&rt, "take me back") && shows(&rt, "doomed answer") && shows(&rt, "Step interrupted"));
    }

    #[test]
    fn committed_drops_the_rows_and_what_hung_off_them() {
        let mut rt = app(None);
        rt.update(|s, _| {
            s.glows.insert("msg_3".into(), 0.0);
            s.inboxes.entry(SID.into()).or_default().upsert(opencode_kit::model::InboxItem {
                id: "msg_9".into(),
                text: "queued behind".into(),
                delivery: opencode_kit::model::Delivery::Queue,
                files: vec![],
            });
        });
        event(&mut rt, "session.revert.staged", json!({ "sessionID": SID, "revert": { "messageID": "msg_3" } }));
        event(&mut rt, "session.revert.committed", json!({ "sessionID": SID, "to": "msg_3" }));
        assert!(shows(&rt, "keep me") && shows(&rt, "kept answer"));
        assert!(!shows(&rt, "take me back") && !shows(&rt, "doomed answer") && !shows(&rt, "Step interrupted"));
        assert_eq!(rt.state.transcripts[SID].messages.len(), 2);
        assert!(rt.state.session(SID).unwrap().revert.is_none());
        assert!(rt.state.glows.is_empty());
        assert!(rt.state.inboxes[SID].items.is_empty());
    }

    #[test]
    fn a_snapshot_mid_revert_opens_hidden() {
        let rt = app(Some("msg_3"));
        assert!(shows(&rt, "kept answer"));
        assert!(!shows(&rt, "take me back") && !shows(&rt, "Step interrupted"));
    }
}
