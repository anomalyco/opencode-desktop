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
