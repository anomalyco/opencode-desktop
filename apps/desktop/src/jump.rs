//! "Jump to latest": the arrow over the timeline's bottom edge while you read more than a screen
//! up (`session/timeline/virtualizer.tsx`, `interaction.ts`). Pressing it glides to the end, and
//! the timeline follows new content again once it lands.
use rune::{Curve, Easing, El, Icon, Shadow, Tx, col, el, icon, row};

use crate::App;
use crate::palette::Pal;

/// Desktop's `jumpThreshold`: more than the viewport's height, and at least 400 above the end.
const THRESHOLD: f32 = 400.0;
/// Its bottom edge sits this far above the timeline's (`bottom: inset + 32px`).
const LIFT: f32 = 32.0;
/// It rises this far as it shows (`translate-y-1`).
const RISE: f32 = 4.0;

static ARROW_DOWN: Icon = Icon {
    name: "arrow-down",
    view: 16.0,
    body: r#"<path d="M12.3333 8.66665L8 13L3.66667 8.66665M8 12.6667V2.83332" stroke="currentColor" stroke-linecap="square"/>"#,
};

/// Records whether session `id`'s timeline stands far enough up to show the button.
fn far(id: &str) -> impl Fn(&mut App, bool) + 'static {
    let id = id.to_owned();
    move |s: &mut App, far| {
        if far { s.far_from_bottom.insert(id.clone()); } else { s.far_from_bottom.remove(&id); }
    }
}

/// The timeline `scroll` with the button floating over its bottom edge, so it rides up as the
/// composer or the docks above it grow.
pub fn over(s: &App, p: &Pal, id: &str, scroll: El) -> El {
    let shown = s.far_from_bottom.contains(id);
    let curve = if shown {
        Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0))
    } else {
        Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0))
    };
    let rise = if shown || s.reduced_motion { 0.0 } else { RISE };
    let target = format!("scroll-{id}");
    let mut button = el()
        .key("jump-latest")
        .role("button")
        .label("Jump to latest")
        .size(32.0, 28.0)
        .radius(8.0)
        .center()
        .fill(p.bg_base.alpha(0.92))
        .border(0.5, p.ring)
        .box_shadow(Shadow { y: 2.0, blur: 4.0, color: p.shadow_far, ..Shadow::default() })
        .box_shadow(Shadow { y: 1.0, blur: 2.0, spread: -1.0, color: p.shadow, ..Shadow::default() })
        .box_shadow(Shadow { y: 2.0, blur: 8.0, color: p.bg_base, ..Shadow::default() })
        .hover(p.hover)
        .press(p.pressed)
        .opacity(if shown { 1.0 } else { 0.0 })
        .opacity_motion(curve)
        .offset(0.0, rise)
        .offset_motion(curve)
        .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.scroll_to_bottom(target.clone()))
        .child(icon(&ARROW_DOWN, 16.0).color(p.text));
    if !shown {
        button = button.inert();
    }
    col()
        .key("timeline-area")
        .grow(1.0)
        .shrink()
        .w_full()
        .child(scroll.on_far_from_bottom(THRESHOLD, far(id)))
        .child(row().key("jump-lane").pin(Some(0.0), None, Some(0.0), Some(LIFT)).center().child(button))
}

#[cfg(test)]
mod tests {
    use rune::{ElementInfo, Env, Runtime};

    use crate::App;

    const DT: f64 = 1.0 / 120.0;

    fn button(rt: &Runtime<App>) -> ElementInfo {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("jump-latest")).unwrap()
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
        assert!(!rt.state.far_from_bottom.contains("bench") && button(&rt).opacity == 0.0, "200 up is near the end");

        for _ in 0..6 {
            rt.wheel(x, y, 400.0);
            rt.frame(DT);
        }
        rest(&mut rt);
        assert!(rt.state.far_from_bottom.contains("bench"), "more than a screen up");
        let b = button(&rt);
        assert!((b.opacity - 1.0).abs() < 1e-3 && !b.inert, "shown: {b:?}");
        // Centred over the timeline, 32 above its bottom edge.
        let v = viewport(&rt);
        assert!(((b.rect[0] + b.rect[2] / 2.0) - (v[0] + v[2] / 2.0)).abs() < 0.5, "{:?} in {v:?}", b.rect);
        assert!(((v[1] + v[3]) - (b.rect[1] + b.rect[3]) - 32.0).abs() < 0.5, "{:?} in {v:?}", b.rect);
        assert_eq!((b.rect[2], b.rect[3]), (32.0, 28.0));

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
        assert!(!rt.state.far_from_bottom.contains("bench") && button(&rt).opacity == 0.0, "gone at the bottom");

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
}
