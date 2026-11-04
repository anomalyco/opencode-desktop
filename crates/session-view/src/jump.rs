//! "Jump to latest": the arrow over the timeline's bottom edge while you read more than a screen
//! up (`session/timeline/virtualizer.tsx`, `interaction.ts`). Pressing it glides to the end, and
//! the timeline follows new content again once it lands.
use std::collections::HashSet;

use rune::{Anim, El, Icon, Tx, col, el, icon, row, spacer};
use rune_kit::ScrollBar;

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{DOCK_SIDE, DOCK_W, activity_ring};

/// Desktop's `jumpThreshold`: more than the viewport's height, and at least 400 above the end.
const THRESHOLD: f32 = 400.0;
/// Centred, its bottom edge sits this far above the timeline's (`bottom: inset + 32px`).
const LIFT: f32 = 32.0;
/// At the right, the gap the jump button and the subagent capsule leave above the composer.
pub const GAP: f32 = 8.0;
/// The capsule's lane: its 28 pt chip and that gap, kept clear under the timeline's last row.
pub const CAPSULE_LANE: f32 = 28.0 + GAP;

/// The room the pills take over the composer: their lane while any shows.
pub fn lane(pills: bool) -> f32 {
    if pills { CAPSULE_LANE } else { 0.0 }
}

/// The timeline's fade over the composer (p248): transparent at the composer's top edge, under
/// the pills' `lane`, and whole the "Composer fade" knob above them, linear in between.
pub fn fade<S: TimelineHost>(s: &S, lane: f32) -> f32 {
    lane + s.knobs().get(&crate::knobs::COMPOSER_FADE)
}

static ARROW_DOWN: Icon = Icon {
    name: "arrow-down",
    view: 16.0,
    body: r#"<path d="M12.3333 8.66665L8 13L3.66667 8.66665M8 12.6667V2.83332" stroke="currentColor" stroke-linecap="square"/>"#,
};

/// Where each session's timeline stands against its end.
#[derive(Default)]
pub struct Ends {
    /// Sessions whose timeline you're reading more than a screen up.
    far: HashSet<String>,
    /// Sessions whose timeline doesn't show its end, where nothing lands in view.
    off_end: HashSet<String>,
}

impl Ends {
    pub fn far(&self, id: &str) -> bool {
        self.far.contains(id)
    }

    pub fn at_end(&self, id: &str) -> bool {
        !self.off_end.contains(id)
    }

    /// A timeline that mounts afresh starts on its end.
    pub fn forget(&mut self, id: &str) {
        self.far.remove(id);
        self.off_end.remove(id);
    }

    pub fn set_far(&mut self, id: &str, far: bool) {
        if far {
            self.far.insert(id.to_owned());
        } else {
            self.far.remove(id);
        }
    }

    pub fn set_at_end(&mut self, id: &str, at: bool) {
        if at {
            self.off_end.remove(id);
        } else {
            self.off_end.insert(id.to_owned());
        }
    }
}

/// Records whether session `id`'s timeline stands far enough up to show the button.
fn far<S: TimelineHost>(id: &str) -> impl Fn(&mut S, bool) + 'static {
    let id = id.to_owned();
    move |s: &mut S, far| s.ends_mut().set_far(&id, far)
}

/// Records whether session `id`'s timeline shows its end.
fn at_end<S: TimelineHost>(id: &str) -> impl Fn(&mut S, bool) + 'static {
    let id = id.to_owned();
    move |s: &mut S, at| s.ends_mut().set_at_end(&id, at)
}

/// The button as it stands: frosted glass, the timeline blurred at 2 pt under bg-base at 92%
/// (`backdrop-blur-[2px]`), raised, with a soft bg-base halo under it (`0 2px 8px`).
pub fn face(p: &Pal, radius: f32) -> El {
    let [far, near, ring] = p.raised;
    el()
        .key("jump-latest")
        .role("button")
        .label("Jump to latest")
        .size(28.0, 28.0)
        .radius(radius)
        .center()
        .backdrop_blur(2.0)
        .fill(p.frost)
        // Bottom first: CSS paints its first shadow on top.
        .shadow(2.0, 8.0, p.bg_base)
        .box_shadow(ring)
        .box_shadow(near)
        .box_shadow(far)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(Anim::INSTANT, Anim::INSTANT)
        .child(icon(&ARROW_DOWN, 16.0).color(p.text))
}

/// The timeline `scroll` with the button and the `pills` (the subagent capsule, the todo pill)
/// floating over its bottom edge. The timeline runs down to the composer and fades out into its
/// top edge, under the pills (p248); at its end its last row stands clear over them (t1129), and
/// they ride up as the docks under it grow. At the right (the ⌘⇧K Composer default) the button
/// ends where the composer ends, on the capsule's row; centred, it's Desktop's.
pub fn over<S: TimelineHost>(s: &S, p: &Pal, id: &str, scroll: El, pills: Vec<El>) -> El {
    let shown = s.ends().far(id);
    // Timeline knobs: 0.15 s in, 0.11 s out, rising 4 pt as it shows (`translate-y-1`).
    let curve = crate::knobs::jump(s.knobs(), shown);
    let rise = if shown || s.reduced_motion() { 0.0 } else { s.knobs().get(&crate::knobs::JUMP_RISE) };
    let target = format!("scroll-{id}");
    let mut button = face(p, s.knobs().get(&crate::knobs::CAPSULE_RADIUS))
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(curve)
        .offset(0.0, rise)
        .offset_motion(curve)
        .on_press_tx(move |_: &mut S, tx: &mut Tx| tx.scroll_to_bottom(target.clone()));
    if !shown {
        button = button.inert();
    }
    let edge = fade(s, lane(!pills.is_empty()));
    let lane = row().key("activity-lane").w_full().max_w(DOCK_W).center_y().children(pills).child(spacer());
    let (lane, centred) = if s.knobs().get(&crate::knobs::JUMP_PLACE) as usize == 1 {
        (lane, Some(row().key("jump-centre").pin(Some(0.0), None, Some(0.0), Some(LIFT)).center().child(button)))
    } else {
        (lane.child(button), None)
    };
    col()
        .key("timeline-area")
        .grow(1.0)
        .shrink()
        .w_full()
        // Its own bottom fade, a fixed one around the scroll's (which a wider one outranks):
        // under the timeline's last row at its end there's only padding.
        // Rune's scroll bar, under the jump button and pills, which keep their presses where
        // they overlap it.
        .child(col().key("timeline-fade").grow(1.0).shrink().w_full().clip_fade_linear(0.0, 0.0, 0.0, edge)
            .child(scroll.on_far_from_bottom(THRESHOLD, far::<S>(id)).on_at_bottom(at_end::<S>(id))
                // Its wrapper isn't a scroll container: from no height, it takes only the room
                // there is, not the content's.
                .scroll_bar(crate::palette::kit(p)).h(0.0)))
        .child(row().key("jump-lane").pin(Some(0.0), None, Some(0.0), Some(GAP)).px(DOCK_SIDE).center().child(lane))
        .children(centred)
        // Over the timeline's top edge, not in it: a row there would move what's on screen.
        .children(s.loading_older(id).then(|| row().key("older-loading").pin(Some(0.0), Some(12.0), Some(0.0), None).center()
            .child(activity_ring(p))))
}
