//! The event stream drops and comes back: the page holds still, and "Reconnecting…" shows once
//! the drop lasts and leaves at once when the stream is back (`crate::reconnect`).

use super::Run;

pub fn run(r: &mut Run) {
    r.act("the stream drops", |rt| rt.update(|s, tx| _ = crate::reconnect::dropped(s, "events: Connection refused", tx)));
    r.play(crate::reconnect::QUIET + 0.5);
    r.act("the stream is back", |rt| rt.update(|s, _| crate::reconnect::back(s)));
    r.play(0.5);
}
