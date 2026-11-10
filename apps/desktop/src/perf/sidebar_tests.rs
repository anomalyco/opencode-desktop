use super::*;
use crate::layout_state::{Shell, TabLayout};
use opencode_kit::model::{Project, SessionInfo};

fn sidebar() -> Runtime<App> {
    let mut app = fixture();
    // This page has finished. Only the four other tabs still have runs.
    app.transcripts.get_mut("bench").unwrap().messages.pop();
    app.layout = Shell::at_rest(TabLayout::Vertical);
    for i in 0..24 {
        let id = format!("project-{i}");
        let root = format!("/source/{id}");
        let dirs: Vec<_> = (0..40).map(|j| format!("/trees/{id}/{j}")).collect();
        app.apply_worktree_event("worktree.listed", &serde_json::json!({"projectID":id,"directories":dirs}));
        app.projects.push(Project { id: id.clone(), name: id.clone(), directory: root, active: 0, icon: Default::default() });
        if i >= 14 {
            let sid = if i == 23 { "bench".into() } else { format!("tab-{i}") };
            app.sessions.push(SessionInfo { id: sid.clone(), title: sid.clone(), directory: dirs[39].clone(), project: id, ..Default::default() });
            app.tabs.push(crate::Tab::session(&sid, &sid, &dirs[39]));
            if i < 18 { app.running.insert(sid); }
        }
    }
    let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    rt
}

#[cfg(feature = "frame-profile")]
#[test]
#[ignore = "release fixture timing; run --release --features frame-profile --ignored --nocapture"]
fn sidebar_update_benchmark() {
    use std::time::Instant;
    let mut rt = sidebar();
    let (mut wall, mut cpu) = (Vec::new(), Vec::new());
    for i in 0..320 {
        let start = Instant::now();
        let first = rune::thread_cpu_ms();
        rt.update(|_, _| {});
        rt.frame(1.0 / 60.0);
        if i >= 32 {
            cpu.push(rune::thread_cpu_ms() - first);
            wall.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    for (name, mut samples) in [("wall", wall), ("cpu", cpu)] {
        samples.sort_by(f64::total_cmp);
        println!("METRIC sidebar_update_{name}_ms p50={:.3} p90={:.3}", samples[144], samples[259]);
    }
}

#[test]
fn sidebar_only_draws_visible_activity() {
    let mut rt = sidebar();
    for (open, busy) in [(true, true), (false, true), (true, false)] {
        rt.update(|s, _| { s.sidebar_open = open; if !busy { s.running.clear(); } });
        for _ in 0..120 { rt.frame(1.0 / 60.0); }
        let before = rt.stats.commits;
        for _ in 0..60 {
            assert!(!rt.frame(1.0 / 60.0), "a radar should sleep between its 30 Hz ticks");
            assert_eq!(rt.wake_deadline().is_some(), open && busy, "open={open} busy={busy}");
        }
        assert_eq!(rt.stats.commits, before, "activity should repaint without rebuilding");
    }
    let before = rt.stats.commits;
    let (sender, inbox) = std::sync::mpsc::channel();
    rt.state.inbox = Some(inbox);
    for _ in 0..180 {
        sender.send(opencode_kit::client::Incoming::Event { ty: "session.text.delta".into(), data: serde_json::json!({
            "sessionID":"tab-15", "assistantMessageID":"stream", "ordinal":0, "delta":" word"
        }) }).unwrap();
        assert!(!rt.frame(1.0 / 60.0));
    }
    assert_eq!(rt.stats.commits, before, "background timestamps must not rebuild an idle session");
    assert!(rt.state.session("tab-15").unwrap().updated > 0);
}
