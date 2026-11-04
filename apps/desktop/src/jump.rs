//! "Jump to latest": the arrow over the timeline's bottom edge while you read more than a screen
//! up (`session/timeline/virtualizer.tsx`, `interaction.ts`). Pressing it glides to the end, and
//! the timeline follows new content again once it lands.
use std::collections::HashSet;

use rune::{Anim, El, Icon, Tx, col, el, icon, row, spacer};

use crate::App;
use crate::palette::Pal;

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
/// the pills' `lane`, and whole the "Composer fade" knob above them.
pub fn fade(s: &App, lane: f32) -> f32 {
    lane + s.knobs.get(&crate::knobs::COMPOSER_FADE)
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
}

/// Records whether session `id`'s timeline stands far enough up to show the button.
fn far(id: &str) -> impl Fn(&mut App, bool) + 'static {
    let id = id.to_owned();
    move |s: &mut App, far| {
        if far { s.ends.far.insert(id.clone()); } else { s.ends.far.remove(&id); }
    }
}

/// Records whether session `id`'s timeline shows its end.
fn at_end(id: &str) -> impl Fn(&mut App, bool) + 'static {
    let id = id.to_owned();
    move |s: &mut App, at| {
        if at { s.ends.off_end.remove(&id); } else { s.ends.off_end.insert(id.clone()); }
    }
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
pub fn over(s: &App, p: &Pal, id: &str, scroll: El, pills: Vec<El>) -> El {
    let shown = s.ends.far(id);
    // Timeline knobs: 0.15 s in, 0.11 s out, rising 4 pt as it shows (`translate-y-1`).
    let curve = crate::knobs::jump(&s.knobs, shown);
    let rise = if shown || s.reduced_motion { 0.0 } else { s.knobs.get(&crate::knobs::JUMP_RISE) };
    let target = format!("scroll-{id}");
    let mut button = face(p, s.knobs.get(&crate::knobs::CAPSULE_RADIUS))
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(curve)
        .offset(0.0, rise)
        .offset_motion(curve)
        .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.scroll_to_bottom(target.clone()));
    if !shown {
        button = button.inert();
    }
    let edge = fade(s, lane(!pills.is_empty()));
    let lane = row().key("activity-lane").w_full().max_w(crate::view::DOCK_W).center_y().children(pills).child(spacer());
    let (lane, centred) = if s.knobs.get(&crate::knobs::JUMP_PLACE) as usize == 1 {
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
        .child(col().key("timeline-fade").grow(1.0).shrink().w_full().clip_fade(0.0, 0.0, 0.0, edge)
            .child(scroll.on_far_from_bottom(THRESHOLD, far(id)).on_at_bottom(at_end(id))))
        .child(row().key("jump-lane").pin(Some(0.0), None, Some(0.0), Some(GAP)).px(crate::view::DOCK_SIDE).center().child(lane))
        .children(centred)
        // Over the timeline's top edge, not in it: a row there would move what's on screen.
        .children(s.pages.loading_older(id).then(|| row().key("older-loading").pin(Some(0.0), Some(12.0), Some(0.0), None).center()
            .child(crate::view::activity_ring(p))))
}

#[cfg(test)]
mod tests {
    use rune::{ElementInfo, Env, Runtime};

    use crate::App;

    const DT: f64 = 1.0 / 120.0;

    fn button(rt: &Runtime<App>) -> ElementInfo {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("jump-latest")).unwrap()
    }

    fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap().rect
    }

    fn viewport(rt: &Runtime<App>) -> [f32; 4] {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("scroll-bench")).unwrap().rect
    }

    /// Text drawn inside the viewport, by path: its top edge.
    fn shown_text(rt: &Runtime<App>) -> Vec<(String, f32, f32)> {
        let v = viewport(rt);
        rt.elements().into_iter()
            .filter(|e| e.text.as_deref().is_some_and(|t| !t.trim().is_empty()) && !e.exiting && e.opacity > 0.0)
            .filter(|e| e.path.contains("scroll-bench") && e.rect[1] < v[1] + v[3] && e.rect[1] + e.rect[3] > v[1])
            .map(|e| (e.path, e.rect[1], e.rect[1] + e.rect[3]))
            .collect()
    }

    fn rest(rt: &mut Runtime<App>) {
        for _ in 0..240 { if !rt.frame(DT) { break; } }
    }

    /// The real session page over a 1,600-message session (only its window of rows mounts).
    #[test]
    fn jump_to_latest_shows_far_up_and_glides_to_a_following_bottom() {
        let mut rt = Runtime::new(crate::perf::fixture(), crate::view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rest(&mut rt);
        assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(true), "a session opens on its end");
        assert_eq!(button(&rt).opacity, 0.0);
        assert!(button(&rt).inert, "hidden, it takes no presses");

        let v = viewport(&rt);
        let (x, y) = (v[0] + v[2] / 2.0, v[1] + v[3] / 2.0);
        // Within a screen of the end it stays hidden.
        rt.wheel(x, y, 200.0);
        rest(&mut rt);
        assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(false));
        assert!(!rt.state.ends.far("bench") && button(&rt).opacity == 0.0, "200 up is near the end");

        for _ in 0..6 {
            rt.wheel(x, y, 400.0);
            rt.frame(DT);
        }
        rest(&mut rt);
        assert!(rt.state.ends.far("bench"), "more than a screen up");
        let b = button(&rt);
        assert!((b.opacity - 1.0).abs() < 1e-3 && !b.inert, "shown: {b:?}");
        // At the composer's right end, the capsule's gap above it, the capsule's size.
        let composer = rect(&rt, "composer");
        assert!(((b.rect[0] + b.rect[2]) - (composer[0] + composer[2])).abs() < 0.5, "{:?} vs composer {composer:?}", b.rect);
        assert!((composer[1] - (b.rect[1] + b.rect[3]) - 8.0).abs() < 0.5, "{:?} vs composer {composer:?}", b.rect);
        assert_eq!((b.rect[2], b.rect[3]), (28.0, 28.0));

        // A running subagent's capsule takes the same row, at the left; the button stays put.
        let at = b.rect;
        rt.update(|s, _| {
            s.sessions.push(opencode_kit::model::SessionInfo { id: "sub".into(), parent: Some("bench".into()), ..Default::default() });
            s.running.insert("sub".into());
        });
        rest(&mut rt);
        let (b, chip) = (button(&rt), rect(&rt, "activity-chip"));
        assert!((b.rect[1] - chip[1]).abs() < 0.5 && (b.rect[3] - chip[3]).abs() < 0.5, "one row: {:?} and chip {chip:?}", b.rect);
        assert!((b.rect[1] - at[1]).abs() < 0.5, "it doesn't climb over the capsule: {:?} was {at:?}", b.rect);
        assert!((chip[0] - rect(&rt, "composer")[0]).abs() < 0.5);
        // The transcript runs on under the capsule and fades out into the composer's top edge (p248).
        let v = viewport(&rt);
        assert!(((v[1] + v[3]) - rect(&rt, "composer")[1]).abs() < 0.5, "{v:?} vs composer");
        assert!(chip[1] > v[1] && chip[1] + chip[3] < v[1] + v[3], "the capsule floats over it: {chip:?} in {v:?}");
        rt.update(|s, _| { s.running.remove("sub"); });
        rest(&mut rt);

        // The Centre knob puts it back where Desktop has it: centred, 32 over the timeline's end.
        rt.update(|s, _| s.knobs.set(&crate::knobs::JUMP_PLACE, 1.0));
        rest(&mut rt);
        let (b, v) = (button(&rt), viewport(&rt));
        assert!(((b.rect[0] + b.rect[2] / 2.0) - (v[0] + v[2] / 2.0)).abs() < 0.5, "{:?} in {v:?}", b.rect);
        assert!(((v[1] + v[3]) - (b.rect[1] + b.rect[3]) - 32.0).abs() < 0.5, "{:?} in {v:?}", b.rect);
        rt.update(|s, _| s.knobs.set(&crate::knobs::JUMP_PLACE, 0.0));
        rest(&mut rt);
        let b = button(&rt);

        // Pressed, the trip plays out without a blank frame and only ever moves down the transcript.
        let (bx, by) = (b.rect[0] + 16.0, b.rect[1] + 14.0);
        rt.pointer_move(bx, by);
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        let mut before = shown_text(&rt);
        let mut frames = 0;
        while rt.scroll_at_bottom("scroll-bench") != Some(true) {
            assert!(frames < 120, "lands within a second");
            rt.frame(DT);
            frames += 1;
            let now = shown_text(&rt);
            assert!(now.len() >= 3, "frame {frames}: the viewport shows rows ({} texts)", now.len());
            let v = viewport(&rt);
            let mut edges: Vec<(f32, f32)> = now.iter().map(|t| (t.1.max(v[1]), t.2.min(v[1] + v[3]))).collect();
            edges.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut reach = v[1];
            let mut gap = 0.0f32;
            for (top, bottom) in edges { gap = gap.max(top - reach); reach = reach.max(bottom); }
            assert!(gap < 160.0, "frame {frames}: no blank band ({gap} pt)");
            for (path, top, _) in &now {
                if let Some((_, was, _)) = before.iter().find(|b| &b.0 == path) {
                    assert!(*top <= was + 0.01, "frame {frames}: {path} moved back {was} -> {top}");
                }
            }
            before = now;
        }
        assert!(frames > 10, "it glides ({frames} frames)");
        rest(&mut rt);
        assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(true));
        assert!(!rt.state.ends.far("bench") && button(&rt).opacity == 0.0, "gone at the bottom");

        // Streaming follows again.
        for _ in 0..10 {
            rt.update(|s, _| {
                s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":"\n\nAnother paragraph of the streamed answer, long enough to wrap onto more lines."}));
            });
            for _ in 0..6 { rt.frame(DT); }
        }
        rest(&mut rt);
        assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(true), "the end follows the stream");
        assert_eq!(button(&rt).opacity, 0.0);
    }

    /// The timeline fades into the composer's top edge, not the capsule's (p248): it runs down to
    /// the composer as that grows, and at its end its last line stands clear over the capsule.
    #[test]
    fn the_timeline_fades_into_the_composer_and_ends_over_the_capsule() {
        for zoom in [1.0, 1.6] {
            let mut app = crate::perf::fixture();
            app.sessions.push(opencode_kit::model::SessionInfo { id: "sub".into(), parent: Some("bench".into()), ..Default::default() });
            app.running.insert("sub".into());
            app.zoom = zoom;
            let mut rt = Runtime::new(app, crate::view::app, (900.0 * zoom, 700.0 * zoom), Env::new(true, 2.0));
            rt.frame(0.0);
            rest(&mut rt);
            assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(true));
            let (v, composer, chip) = (viewport(&rt), rect(&rt, "composer"), rect(&rt, "activity-chip"));
            assert!(((v[1] + v[3]) - composer[1]).abs() < 0.5, "zoom {zoom}: {v:?} ends on the composer {composer:?}");
            let last = shown_text(&rt).iter().map(|t| t.2).fold(f32::MIN, f32::max);
            assert!(last <= chip[1] - 8.0, "zoom {zoom}: the last line ({last}) ends clear over the capsule {chip:?}");
            assert_eq!(super::fade(&rt.state, super::lane(true)), super::CAPSULE_LANE + 16.0);

            rt.update(|s, _| s.composer = rune::TextEdit::new("one\ntwo\nthree\nfour"));
            rest(&mut rt);
            let (v, composer) = (viewport(&rt), rect(&rt, "composer"));
            assert!(((v[1] + v[3]) - composer[1]).abs() < 0.5, "zoom {zoom}: a taller composer: {v:?} vs {composer:?}");
        }
    }

    /// On the capsule's row, over the timeline's fading edge, it takes the press.
    #[test]
    fn beside_the_capsule_it_still_presses() {
        let mut app = crate::perf::fixture();
        app.sessions.push(opencode_kit::model::SessionInfo { id: "sub".into(), parent: Some("bench".into()), ..Default::default() });
        app.running.insert("sub".into());
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rest(&mut rt);
        let v = viewport(&rt);
        for _ in 0..6 {
            rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, 400.0);
            rt.frame(DT);
        }
        rest(&mut rt);
        let b = button(&rt);
        let chip = rect(&rt, "activity-chip");
        assert!((b.rect[1] - chip[1]).abs() < 0.5 && b.rect[1] + b.rect[3] < v[1] + v[3], "over the timeline's edge: {:?} vs {v:?}", b.rect);
        let (bx, by) = (b.rect[0] + 14.0, b.rect[1] + 14.0);
        rt.pointer_move(bx, by);
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rest(&mut rt);
        assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(true));
    }

    /// Back from a subagent's session, the parent opens on its end: no stale button fading out.
    #[test]
    fn back_on_a_timeline_that_opens_on_its_end_it_stays_hidden() {
        let mut app = crate::perf::fixture();
        app.sessions.push(opencode_kit::model::SessionInfo { id: "sub".into(), parent: Some("bench".into()), ..Default::default() });
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rest(&mut rt);
        let v = viewport(&rt);
        for _ in 0..6 {
            rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, 400.0);
            rt.frame(DT);
        }
        rest(&mut rt);
        assert_eq!(button(&rt).opacity, 1.0, "far up");
        rt.update(|s, tx| s.open_child("bench", "sub", "Sub", "general", tx));
        rest(&mut rt);
        rt.update(|s, tx| assert!(s.open_parent(tx)));
        for frame in 0..60 {
            rt.frame(DT);
            assert_eq!(button(&rt).opacity, 0.0, "frame {frame}");
        }
        assert_eq!(rt.scroll_at_bottom("scroll-bench"), Some(true));
    }
}
