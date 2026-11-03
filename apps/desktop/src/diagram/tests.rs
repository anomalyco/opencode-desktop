//! A diagram wider than the column draws whole in its card, and opens full size: it grows out
//! of its card, pinches and pans on the fingers, fits again on ⌘0 and flies back on Esc (p247).

use rune::{ElementInfo, Key, KeyEvent, MagnifyEvent, Mods, Runtime, WheelEvent};

use crate::App;

const ROW: &str = "t-msg_zzwide1";
const DT: f64 = 1.0 / 60.0;

fn runtime(size: (f32, f32), zoom: f32) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = zoom;
    super::stories::add(&mut app);
    let mut rt = app.into_runtime_sized(size);
    frames(&mut rt, 90);
    rt
}

fn frames(rt: &mut Runtime<App>, n: usize) {
    for _ in 0..n {
        rt.frame(DT);
    }
}

fn find(rt: &Runtime<App>, key: &str, within: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains(within))
}

fn get(rt: &Runtime<App>, key: &str, within: &str) -> ElementInfo {
    find(rt, key, within).unwrap_or_else(|| panic!("no {key} in {within}"))
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5 && inner[1] >= outer[1] - 0.5 && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5 && inner[1] + inner[3] <= outer[1] + outer[3] + 0.5
}

fn near(a: [f32; 4], b: [f32; 4], by: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= by)
}

/// The opacity the reply's card draws at: its frame's, which hides it while it's open.
fn card_opacity(rt: &Runtime<App>) -> f32 {
    let els = rt.elements();
    let card = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some("card") && e.path.contains(ROW)).expect("the card");
    els[card.parent.expect("its frame")].opacity
}

#[test]
fn a_wide_sequence_diagram_draws_whole_in_its_card() {
    for zoom in [1.0, 1.6] {
        for width in [560.0, 1800.0] {
            let rt = runtime((width * zoom, 760.0 * zoom), zoom);
            let card = get(&rt, "card", ROW).rect;
            let diagram = get(&rt, "diagram", ROW).rect;
            let column = get(&rt, &format!("{ROW}-0"), "").rect;
            let what = format!("window {width} zoom {zoom}: column {column:?} card {card:?} diagram {diagram:?}");
            assert!(inside(diagram, card) && inside(card, column), "{what}");
            let labels: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains(ROW) && e.text.as_deref().is_some_and(|t| t.contains("Browser") || t.contains("Audit"))).collect();
            assert!(labels.len() >= 3, "every lane draws: {what}");
            for l in labels {
                assert!(inside(l.rect, card), "{:?} leaves the card: {what}", l.text);
            }
        }
    }
}

#[test]
fn it_opens_from_its_card_zooms_and_pans_on_the_fingers_and_flies_back() {
    for zoom in [1.0, 1.6] {
        let (w, h) = (1280.0 * zoom, 820.0 * zoom);
        let mut rt = runtime((w, h), zoom);
        let card = get(&rt, "card", ROW).rect;
        rt.pointer_move(card[0] + card[2] / 2.0, card[1] + card[3] / 2.0);
        frames(&mut rt, 2);
        let (bx, by) = rt.find("diagram-open").expect("the expand button");
        rt.pointer_move(bx, by);
        rt.pointer_down(bx, by);
        rt.pointer_up(bx, by);
        rt.frame(DT);
        let first = get(&rt, "diagram-lightbox", "").rect;
        let what = format!("zoom {zoom}: card {card:?}, first frame {first:?}");
        assert!((first[0] - card[0]).abs() < 30.0 && (first[1] - card[1]).abs() < 30.0 && (first[2] / card[2] - 1.0).abs() < 0.15, "it starts on its card: {what}");
        assert_eq!(card_opacity(&rt), 0.0, "the card it left hides: {what}");

        frames(&mut rt, 60);
        let open = get(&rt, "diagram-lightbox", "").rect;
        let what = format!("zoom {zoom}: card {card:?}, open {open:?}");
        assert!(((open[0] + open[2] / 2.0) - w / 2.0).abs() < 1.0 && ((open[1] + open[3] / 2.0) - h / 2.0).abs() < 1.0, "centred: {what}");
        assert!(open[2] > card[2] * 1.2 && inside(open, [0.0, 0.0, w, h]), "bigger, and in the window: {what}");

        // A pinch: the point under the fingers stays under them, in the same frame.
        let p = [open[0] + open[2] * 0.3, open[1] + open[3] * 0.4];
        rt.magnify_event(MagnifyEvent::new(0.5).at(p[0], p[1]));
        rt.frame(DT);
        let pinched = get(&rt, "diagram-lightbox", "").rect;
        let u = |r: [f32; 4]| [(p[0] - r[0]) / r[2], (p[1] - r[1]) / r[3]];
        let what = format!("zoom {zoom}: open {open:?}, pinched {pinched:?}");
        assert!((pinched[2] / open[2] - 1.5).abs() < 0.01, "1.5 times as big at once: {what}");
        assert!((u(pinched)[0] - u(open)[0]).abs() < 0.002 && (u(pinched)[1] - u(open)[1]).abs() < 0.002, "about the fingers: {what}");

        // Two fingers: it moves as far as they do, in the same frame.
        rt.wheel_event(WheelEvent::new(30.0, -20.0).at(p[0], p[1]).precise(true));
        rt.frame(DT);
        let panned = get(&rt, "diagram-lightbox", "").rect;
        let what = format!("zoom {zoom}: pinched {pinched:?}, panned {panned:?}");
        assert!((panned[0] - pinched[0] - 30.0).abs() < 0.5 && (panned[1] - pinched[1] + 20.0).abs() < 0.5, "on the fingers: {what}");

        rt.key(KeyEvent { key: Key::Command('0'), mods: Mods { cmd: true, ..Default::default() } });
        frames(&mut rt, 60);
        assert!(near(get(&rt, "diagram-lightbox", "").rect, open, 1.0), "⌘0 fits it again, zoom {zoom}");
        assert!((rt.state.zoom - zoom).abs() < 1e-4, "and leaves the app's zoom alone");

        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        frames(&mut rt, 22);
        let landing = get(&rt, "diagram-lightbox", "").rect;
        assert!(near(landing, card, 2.0), "it flies back onto its card, zoom {zoom}: {landing:?} {card:?}");
        frames(&mut rt, 30);
        assert!(find(&rt, "diagram-lightbox", "").is_none() && !rt.state.diagram.is_open(), "then goes, zoom {zoom}");
        assert_eq!(card_opacity(&rt), 1.0, "and the card is back, zoom {zoom}");
    }
}

#[test]
fn space_opens_the_diagram_under_the_pointer_and_closes_it() {
    let mut rt = runtime((1280.0, 820.0), 1.0);
    let card = get(&rt, "card", ROW).rect;
    rt.pointer_move(card[0] + card[2] / 2.0, card[1] + card[3] / 2.0);
    frames(&mut rt, 2);
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    frames(&mut rt, 30);
    assert!(rt.state.diagram.is_open(), "Space opens it");
    rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Mods::default() });
    frames(&mut rt, 40);
    assert!(!rt.state.diagram.is_open(), "and Space closes it");
}

#[test]
fn a_click_outside_closes_it_and_a_double_click_on_it_zooms_in() {
    let mut rt = runtime((1280.0, 820.0), 1.0);
    let card = get(&rt, "card", ROW).rect;
    let (x, y) = (card[0] + card[2] / 2.0, card[1] + card[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(&mut rt, 60);
    let open = get(&rt, "diagram-lightbox", "").rect;
    let (cx, cy) = (open[0] + open[2] / 2.0, open[1] + open[3] / 2.0);
    for _ in 0..2 {
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
    }
    frames(&mut rt, 60);
    let zoomed = get(&rt, "diagram-lightbox", "").rect;
    assert!((zoomed[2] / open[2] - 2.0).abs() < 0.01, "a double-click doubles it: {open:?} {zoomed:?}");
    for _ in 0..2 {
        rt.pointer_down(cx, cy);
        rt.pointer_up(cx, cy);
    }
    frames(&mut rt, 60);
    assert!(near(get(&rt, "diagram-lightbox", "").rect, open, 1.0), "and another fits it again");
    rt.pointer_down(4.0, 4.0);
    rt.pointer_up(4.0, 4.0);
    frames(&mut rt, 60);
    assert!(!rt.state.diagram.is_open(), "a click outside closes it");
}
