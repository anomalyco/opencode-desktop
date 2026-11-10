//! The header and the composer's toolbar give way in a narrow window, never overlapping or
//! clipping, from the narrowest window the app allows up (p263).
use rune::{ElementInfo, Runtime};

use crate::App;

/// Rune's smallest window (`windows.rs`'s `DEFAULT_MIN_SIZE`; the app sets none).
const MIN_W: f32 = 420.0;

fn runtime(width: f32, zoom: f32, sidebar: bool) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = zoom;
    app.sidebar_open = sidebar;
    let mut rt = Runtime::new(app, super::app, (width, 760.0), rune::Env::new(true, 1.0));
    settle(&mut rt);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

fn live(rt: &Runtime<App>) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| !e.exiting).collect()
}

fn one<'a>(els: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

/// The elements directly inside `parent`, with some width.
fn children<'a>(els: &'a [ElementInfo], parent: &ElementInfo) -> Vec<&'a ElementInfo> {
    let prefix = format!("{}/", parent.path);
    els.iter().filter(|e| e.path.strip_prefix(&prefix).is_some_and(|rest| !rest.contains('/')) && e.rect[2] > 0.5).collect()
}

fn name(e: &ElementInfo) -> String {
    e.key.clone().or(e.text.clone()).or(e.label.clone()).unwrap_or_else(|| e.path.clone())
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] + 0.5 < b[0] + b[2] && b[0] + 0.5 < a[0] + a[2] && a[1] + 0.5 < b[1] + b[3] && b[1] + 0.5 < a[1] + a[3]
}

fn inside(inner: [f32; 4], outer: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5 && inner[0] + inner[2] <= outer[0] + outer[2] + 0.5
}

fn apart(pieces: &[&ElementInfo], what: &str) {
    for (i, a) in pieces.iter().enumerate() {
        for b in &pieces[i + 1..] {
            assert!(!overlap(a.rect, b.rect), "{what}: {} {:?} overlaps {} {:?}", name(a), a.rect, name(b), b.rect);
        }
    }
}

/// The header's pieces sit apart inside the header; the toolbar's buttons sit apart inside the
/// composer, with Send whole. Below Desktop's 768 pt breakpoint the header hides under the view
/// switcher (`screen.tsx`'s `hideHeader`), and its usage is the context plugin's ring among its
/// actions (3609b1dd).
fn check(width: f32, zoom: f32, sidebar: bool) {
    let rt = runtime(width, zoom, sidebar);
    let els = live(&rt);
    let what = format!("window {width} zoom {zoom} sidebar {sidebar}");

    if rt.state.narrow() {
        assert!(els.iter().all(|e| e.key.as_deref() != Some("header")), "{what}: a narrow window hides the header");
    } else {
        let header = one(&els, "header");
        let mut pieces = children(&els, one(&els, "crumbs"));
        pieces.extend(children(&els, one(&els, "header-actions")));
        for p in &pieces {
            assert!(inside(p.rect, header.rect), "{what}: {} {:?} leaves the header {:?}", name(p), p.rect, header.rect);
        }
        apart(&pieces, &what);
    }

    let composer = one(&els, "composer");
    let tools: Vec<_> = children(&els, one(&els, "toolbar")).into_iter().filter(|e| e.key.as_deref() != Some("room")).collect();
    for t in &tools {
        assert!(inside(t.rect, composer.rect), "{what}: {} {:?} leaves the composer {:?}", name(t), t.rect, composer.rect);
    }
    apart(&tools, &what);
    let send = one(&els, "submit");
    assert!((send.rect[2] - 28.0 * zoom).abs() < 0.5, "{what}: Send whole {:?}", send.rect);
    assert!(tools.iter().any(|t| t.key.as_deref() == Some("model")), "{what}: the model's button stays");
}

#[test]
fn nothing_overlaps_or_clips_at_the_narrowest_window() {
    for zoom in [1.0, 1.6] {
        check(MIN_W, zoom, false);
    }
    check(MIN_W, 1.0, true);
}

/// Every width up to a wide window: the sidebar open at zoom 1.6 only where it leaves the
/// session a usable panel (Air-sized windows included).
#[test]
fn nothing_overlaps_or_clips_at_any_width() {
    for zoom in [1.0, 1.6] {
        for sidebar in [false, true] {
            let mut w = MIN_W;
            while w <= 1480.0 {
                let panel = w / zoom - if sidebar { crate::view::SIDEBAR_W } else { 0.0 } - 16.0;
                if panel >= 140.0 {
                    check(w, zoom, sidebar);
                }
                w += 30.0;
            }
        }
    }
}

/// Narrowing the window a few points a frame, the header and toolbar land each layout at once:
/// every frame shows where they settle, with no glide when an item drops or folds.
#[test]
fn drops_and_folds_land_at_once() {
    let zoom = 1.6;
    let mut rt = runtime(900.0, zoom, false);
    let keys = ["title", "header-more", "usage", "model", "effort", "submit", "mic"];
    let place = |rt: &Runtime<App>| -> Vec<(String, [f32; 4])> {
        let els = live(rt);
        keys.iter().filter_map(|k| els.iter().find(|e| e.key.as_deref() == Some(*k)).map(|e| (k.to_string(), e.rect))).collect()
    };
    let mut w = 900.0;
    while w > MIN_W {
        w -= 8.0;
        rt.set_viewport((w, 760.0), 1.0);
        rt.frame(1.0 / 60.0);
        let now = place(&rt);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let settled = place(&rt);
        assert_eq!(now.iter().map(|x| &x.0).collect::<Vec<_>>(), settled.iter().map(|x| &x.0).collect::<Vec<_>>(), "window {w}: the same pieces show");
        for ((k, a), (_, b)) in now.iter().zip(&settled) {
            assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.75), "window {w}: {k} at {a:?}, settles at {b:?}");
        }
    }
}
