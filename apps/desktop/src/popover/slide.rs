//! The lit row's highlight in a list: one fill under the rows that slides to the row the
//! pointer or the arrow keys light, rather than jumping (Kit, t1150: "If it has the highlight
//! around it, it should animate width of movement"). It rides a live offset, so it moves even
//! inside a surface whose other changes land at once. A new filter puts it in place at once.

use std::cell::RefCell;

use rune::{El, Live, Spring, el};
use rune_knobs::Knobs;

use super::Look;
use super::motion::SLIDE;
use crate::palette::Pal;

thread_local! {
    /// Where the open list's highlight is: one list is open at a time.
    static AT: Live<[f32; 2]> = Live::new([0.0, 0.0]);
    /// The filter the highlight last showed under, and how many it has had.
    static SHOWN: RefCell<(String, u64)> = const { RefCell::new((String::new(), 0)) };
}

/// The highlight under a list's rows, at `y` in it, for the list filtered by `query`. A new
/// query mounts a new one in place; under one query it slides.
pub fn highlight(k: &Knobs, p: &Pal, look: Look, y: f32, query: &str) -> El {
    let generation = SHOWN.with_borrow_mut(|(q, n)| {
        if q != query {
            *q = query.to_owned();
            *n += 1;
        }
        *n
    });
    AT.with(|at| at.set([0.0, y]));
    let secs = k.get(&SLIDE);
    let spring = if secs <= 0.0 { Spring::NONE } else { Spring::new(secs, 0.0) };
    AT.with(|at| {
        el().key(format!("lit-{generation}"))
            .absolute(0.0, 0.0)
            .w_full()
            .h(look.row_h)
            .px(look.inset)
            .transition(rune::Transition::none())
            .live_offset(at.smooth(spring))
            .child(el().key("lit-fill").w_full().h_full().radius(look.row_radius).fill(p.row_active))
    })
}
