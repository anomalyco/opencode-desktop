//! Motion-gate scenarios for the right-hand Side Region and Changes (`Review`) panel.

use super::Run;

pub fn side_region(r: &mut Run) {
    r.act("seed side tabs", |rt| {
        rt.update(|s, tx| {
            tx.settle();
            s.open_new_browser_tab("http://localhost:3000/");
            s.open_new_browser_tab("https://github.com/anomalyco/rune");
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.15);
    r.act("close side region", |rt| Run::tap(rt, "side-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.6);
    r.act("reopen side region", |rt| Run::tap(rt, "side-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.6);
    r.act("switch tab", |rt| {
        rt.update(|s, tx| {
            let _ = s.select_adjacent_side_tab(-1, tx);
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.type_prompt();
    r.enter(false);
    r.play_out(1.0);
}

pub fn review(r: &mut Run) {
    r.act("seed review open", |rt| {
        rt.update(|s, tx| {
            s.toggle_side_region(tx);
            tx.settle();
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.15);
    r.act("next file", |rt| Run::tap(rt, "review-next-file"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("toggle file tree", |rt| Run::tap(rt, "review-tree-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("split diff", |rt| Run::tap(rt, "review-style-split"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("unified diff", |rt| Run::tap(rt, "review-style-unified"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("close review", |rt| Run::tap(rt, "side-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.6);
    r.act("reopen review", |rt| Run::tap(rt, "side-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play_out(0.8);
}
pub fn files(r: &mut Run) {
    r.act("seed file open", |rt| {
        rt.update(|s, tx| {
            s.open_workspace_file("src/net/stream.ts", false, Some(12), tx);
            tx.settle();
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.15);
    r.act("toggle file tree", |rt| Run::tap(rt, "files-tree-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("open ⌘P", |rt| {
        rt.update(|s, tx| s.open_file_palette(tx));
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("close ⌘P", |rt| {
        rt.update(|s, tx| {
            tx.instant();
            s.close_file_palette(true);
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("close side region", |rt| Run::tap(rt, "side-toggle"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play_out(0.8);
}

/// Tool steps opening in the side region (`tool_open`): an edit opens Changes, a read switches
/// to Files, the same click shuts it, and Esc puts back the tab that showed before.
pub fn tool_steps(r: &mut Run) {
    r.act("seed edit group open", |rt| {
        rt.update(|s, tx| {
            s.expanded.insert("c-call_edit_50000".into());
            s.expanded.insert("c-call_grep_27000".into());
            tx.settle();
            tx.scroll_key_to("tool-call_edit_50000", 300.0);
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    // The scroll lands a couple of frames on; the page is loaded once it has.
    r.play(0.1);
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.act("open edit step", |rt| Run::tap(rt, "tool-call_edit_50000"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.act("same click shuts it", |rt| Run::tap(rt, "tool-call_edit_50000"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.act("open Files tab", |rt| rt.update(|s, tx| s.open_workspace_file("src/net/backoff.ts", false, None, tx)));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.act("open edit step over it", |rt| Run::tap(rt, "tool-call_edit_50000"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.act("switch to read step", |rt| rt.update(|s, tx| s.press_tool_step("call_read_28000", tx)));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.3);
    r.act("Esc back to Files", |rt| rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() }));
    r.rec.loads.push(r.rec.frames - 1);
    r.play_out(0.6);
}

pub fn terminal_dock(r: &mut Run) {
    use rune::{Key, KeyEvent, Mods};
    let cmd = |c| KeyEvent {
        key: Key::Command(c),
        mods: Mods { cmd: true, logo: true, ..Mods::default() },
    };
    r.act("seed terminal open", |rt| {
        rt.update(|s, tx| {
            s.toggle_dock(tx);
            tx.settle();
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.15);
    r.act("new terminal ⌃⌥T", |rt| {
        rt.key(KeyEvent {
            key: Key::Text("t".into()),
            mods: Mods { ctrl: true, alt: true, ..Mods::default() },
        });
    });
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("switch terminal tab", |rt| Run::tap(rt, "ttab-term_1"));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("close terminal tab ⌘W", |rt| rt.key(cmd('w')));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.2);
    r.act("close terminal ⌘J", |rt| rt.key(cmd('j')));
    r.rec.loads.push(r.rec.frames - 1);
    r.play(0.6);
    r.act("reopen terminal ⌘J", |rt| rt.key(cmd('j')));
    r.rec.loads.push(r.rec.frames - 1);
    r.play_out(0.8);
}
