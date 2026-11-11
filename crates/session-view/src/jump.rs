//! "Jump to latest": the arrow over the timeline's bottom edge while you read more than a screen
//! up (`session/timeline/virtualizer.tsx`, `interaction.ts`). Pressing it glides to the end, and
//! the timeline follows new content again once it lands.
use std::collections::HashMap;

use rune::{Anim, El, Icon, Tx, col, el, icon, row, spacer};
use rune_kit::ScrollBar;

use crate::TimelineHost;
use crate::follow::{self, Effect, Event, Follow};
use crate::palette::Pal;
use crate::style::{DOCK_SIDE, activity_ring};

/// Where the pills over the composer sit (Settings › Preferences › Background activity,
/// `useComposerLaneAlignment`): at the lane's start, centred, or at its end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// Desktop's `jumpThreshold` (`interaction.ts:125`): more than the viewport's height, and at
/// least 400 above the end. Desktop's viewport runs under the header and the composer, so its
/// height is the pane's (`TimelineHost::timeline_frame`), not the rows' window above the composer.
const THRESHOLD: f32 = 400.0;
/// Centred, its bottom edge sits this far above the timeline's (`bottom: inset + 32px`).
const LIFT: f32 = 32.0;
/// At the right, the gap the jump button and the subagent capsule leave above the composer
/// (the pill lane's `pb-2.5`, `composer-lane.tsx:36`).
pub const GAP: f32 = 10.0;
/// A composer pill's height (`[data-composer-pill]`, `composer-lane.css:7`).
pub const CHIP_H: f32 = 32.0;
/// The capsule's lane: its chip and that gap, kept clear under the timeline's last row.
pub const CAPSULE_LANE: f32 = CHIP_H + GAP;

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
    /// Each session's follow state; one not listed follows its end ([`Follow::Following`]).
    follow: HashMap<String, Follow>,
    /// Where each timeline was last drawn, for its place when it shows again.
    pub recall: crate::recall::Recall,
    /// The session coming back to its place, and that place, until its timeline shows.
    due: Option<(String, String, f32)>,
}

impl Ends {
    /// Whether session `id`'s timeline stands more than a screen up, where Jump to latest shows.
    pub fn far(&self, id: &str) -> bool {
        self.follow.get(id).is_some_and(Follow::far)
    }

    /// Whether session `id`'s timeline follows its end, where what lands shows.
    pub fn at_end(&self, id: &str) -> bool {
        self.follow.get(id).is_none_or(Follow::following)
    }

    /// A timeline that mounts afresh starts on its end.
    pub fn forget(&mut self, id: &str) {
        self.follow.remove(id);
    }

    /// Where session `id`'s timeline was read when it last showed, scrolled up: its first row
    /// showing and how far below the top that row's top stood. `None` while it follows its end.
    pub fn place(&self, id: &str) -> Option<(String, f32)> {
        if self.at_end(id) { return None; }
        self.recall.place(id)
    }

    /// Session `id` is opened again. Read scrolled up, it comes back to its place once its
    /// timeline shows ([`Ends::land`]), with Jump to latest as it was; otherwise its timeline
    /// mounts afresh on its end and follows it.
    pub fn reopen(&mut self, id: &str) {
        self.due = self.place(id).map(|(row, at)| (id.to_owned(), row, at));
        if self.due.is_none() {
            self.forget(id);
        }
    }

    /// Session `id`'s timeline shows: put its place back in this commit, so its first frame
    /// draws it there. A row out of the mounted window is mounted first.
    pub fn land(&mut self, id: &str, tx: &mut Tx) {
        let Some((_, row, at)) = self.due.take_if(|(due, ..)| due == id) else { return };
        tx.scroll_into_view_in(format!("scroll-{id}"), row.clone());
        tx.scroll_key_to(row, at);
    }

    /// Steps session `id`'s follow state (`follow::step`); `None` steps a timeline that belongs
    /// to no session as a detached one. Returns the hold to make.
    pub fn step(&mut self, id: Option<&str>, event: Event) -> Option<Effect> {
        let Some(id) = id else { return follow::step(Follow::Detached { far: false }, event).1 };
        let (next, effect) = follow::step(self.follow.remove(id).unwrap_or_default(), event);
        if next != Follow::Following {
            self.follow.insert(id.to_owned(), next);
        }
        effect
    }

    pub fn set_far(&mut self, id: &str, far: bool) {
        self.step(Some(id), Event::Far(far));
    }

    pub fn set_at_end(&mut self, id: &str, at: bool) {
        self.step(Some(id), Event::AtEnd(at));
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
    let [far, near, ring, edge] = p.raised;
    el()
        .key("jump-latest")
        .role("button")
        .label(opencode_i18n::t!("session.messages.jumpToLatest"))
        // `w-8 h-7 rounded-lg` (`virtualizer.tsx:1141`).
        .size(32.0, 28.0)
        .radius(radius)
        .center()
        .backdrop_blur(2.0)
        .fill(p.frost)
        // Bottom first: CSS paints its first shadow on top.
        .shadow(2.0, 8.0, p.bg_base)
        .box_shadow(edge)
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
///
/// `top`: the height of the session header stuck over the timeline's top, which the
/// older-history ring sits under.
pub fn over<S: TimelineHost>(s: &S, p: &Pal, id: &str, scroll: El, pills: Vec<El>, top: f32) -> El {
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
    // A pill's height whatever shows, so the 28 pt button sits where it does beside the 32 pt
    // capsule and doesn't move 2 pt when one comes or goes.
    let lane = row().key("activity-lane").w_full().max_w(crate::style::dock_w()).min_h(CHIP_H).center_y();
    let lane = match s.lane_align() {
        Align::Start => lane.children(pills).child(spacer()),
        Align::End => lane.child(spacer()).children(pills),
        // Centred on the lane whatever sits at its end: over it, out of its flow.
        Align::Center => lane.child(spacer()).children((!pills.is_empty()).then(|| row().key("activity-centre").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).center().children(pills))),
    };
    // Pills at the right end the lane where Desktop's do, at the composer's end, so the button
    // stands centred above the composer there, as Desktop's always does.
    let (lane, centred) = if s.knobs().get(&crate::knobs::JUMP_PLACE) as usize == 1 || s.lane_align() == Align::End {
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
            .child(match s.timeline_frame() {
                Some(frame) => scroll.on_far_from_bottom_under(THRESHOLD, frame, far::<S>(id)),
                None => scroll.on_far_from_bottom(THRESHOLD, far::<S>(id)),
            }.on_at_bottom(at_end::<S>(id))
                .scroll_bar_styled(crate::palette::scroll_bar(p).hover_target())))
        .child(row().key("jump-lane").pin(Some(0.0), None, Some(0.0), Some(GAP)).px(DOCK_SIDE).center().child(lane))
        .children(centred)
        // Over the timeline's top edge, not in it: a row there would move what's on screen.
        .children(s.loading_older(id).then(|| row().key("older-loading").pin(Some(0.0), Some(top + 12.0), Some(0.0), None).center()
            .child(activity_ring(p))))
        // The thumb follows the hover of the whole area, Jump to latest included, as Desktop's
        // `thumbHoverTarget={viewRoot()}` (`virtualizer.tsx:1157-1161`).
        .scroll_bar_hover(&format!("scroll-{id}"))
}
