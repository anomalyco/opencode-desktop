//! The todo pill's cell: the plan shows as the run works, its list opens and closes at once,
//! the todo in hand changes in place, and the pill goes when the plan is done.

use super::Run;
use crate::todo_stories as st;

pub fn todo_pill(r: &mut Run) {
    r.act("the run plans", |rt| {
        rt.update(|s, _| {
            s.running.insert(super::SID.into());
            st::set_step(s, 1);
        })
    });
    r.play(0.6);
    r.act("open the list", |rt| Run::tap(rt, "todo-pill"));
    r.play(0.4);
    r.act("the plan moves on", |rt| rt.update(|s, _| st::set_step(s, 2)));
    r.play(0.4);
    r.act("close it", |rt| Run::tap(rt, "todo-pill"));
    r.play(0.4);
    r.act("the next todo", |rt| rt.update(|s, _| st::set_step(s, 3)));
    r.play(0.4);
    r.act("the plan is done", |rt| rt.update(|s, _| st::set_step(s, st::PLAN.len())));
    r.play_out(0.6);
}
