//! Icon buttons hold their glyph in the middle, measured as drawn: layout from a headless
//! window, ink from the glyph's own raster (p267: Copy's glyph sat 6 pt high in its square).

use rune::{ElementInfo, Env, Icon, Runtime};
use rune_theme_opencode::icons as ic;

use crate::App;

/// How far off a glyph may sit, in device pixels.
const SLACK: f32 = 0.5;

/// Kit's 1× and the 2× screen at the zoom he reads at.
const VIEWS: [(f32, f32); 2] = [(1.0, 1.0), (1.6, 2.0)];

fn window(zoom: f32, scale: f32, setup: impl FnOnce(&mut App)) -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    setup(&mut app);
    app.zoom = zoom;
    app.reduced_motion = true;
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, scale));
    rt.poll = Some(crate::poll);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

/// A session whose last turn ends in text, so its Copy sits under a reply's last line.
fn reply(zoom: f32, scale: f32) -> Runtime<App> {
    window(zoom, scale, |s| s.route = crate::Route::here("ses_fx0004aaaaaaaaaaaaaaaaaa".into()))
}

fn centre(r: [f32; 4]) -> [f32; 2] {
    [r[0] + r[2] / 2.0, r[1] + r[3] / 2.0]
}

fn find<'a>(els: &'a [ElementInfo], key: &str) -> &'a ElementInfo {
    els.iter().rfind(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

/// `icon`'s ink drawn in `rect` at `scale`, in window points: the box of its covered pixels,
/// each edge pulled in by how little its outermost pixels cover.
fn ink(icon: &Icon, rect: [f32; 4], scale: f32) -> [f32; 4] {
    let px = (rect[2] * scale).round() as usize;
    let cov = rune::text::icon_coverage(icon, px as u32);
    let edges = |most: Vec<u8>| {
        let first = most.iter().position(|&c| c > 0).expect("ink");
        let last = most.iter().rposition(|&c| c > 0).unwrap();
        (first as f32 + 1.0 - most[first] as f32 / 255.0, last as f32 + most[last] as f32 / 255.0)
    };
    let (x0, x1) = edges((0..px).map(|x| (0..px).map(|y| cov[y * px + x]).max().unwrap()).collect());
    let (y0, y1) = edges((0..px).map(|y| cov[y * px..(y + 1) * px].iter().copied().max().unwrap()).collect());
    let k = rect[2] / px as f32;
    [rect[0] + x0 * k, rect[1] + y0 * k, (x1 - x0) * k, (y1 - y0) * k]
}

#[test]
fn every_icon_button_holds_its_icon_in_the_middle() {
    let within = |els: &[ElementInfo], mut i: usize, root: usize| loop {
        if i == root {
            break true;
        }
        match els[i].parent {
            Some(p) => i = p,
            None => break false,
        }
    };
    let mut seen = 0;
    for (zoom, scale) in VIEWS {
        for (name, setup) in crate::scenes::all().into_iter().filter(|s| s.0 != "demo") {
            let rt = window(zoom, scale, setup);
            let els = rt.elements();
            for (b, button) in els.iter().enumerate().filter(|(_, e)| e.role == "button" && !e.exiting) {
                let inside: Vec<&ElementInfo> = (0..els.len()).filter(|&i| i != b && within(&els, i, b)).map(|i| &els[i]).collect();
                let icons: Vec<_> = inside.iter().filter(|e| e.role == "icon" && !e.exiting).collect();
                if icons.len() != 1 || inside.iter().any(|e| e.text.as_deref().is_some_and(|t| !t.trim().is_empty())) {
                    continue;
                }
                let ([bx, by], [ix, iy]) = (centre(button.rect), centre(icons[0].rect));
                let off = [(ix - bx) * scale, (iy - by) * scale];
                assert!(off.iter().all(|d| d.abs() <= SLACK), "{name} at zoom {zoom}, {scale}×: {} sits {off:?} px off its middle", button.path);
                seen += 1;
            }
        }
    }
    assert!(seen > 40, "only {seen} icon buttons checked");
}

#[test]
fn copy_and_its_tick_sit_in_the_middle_of_the_square() {
    for (zoom, scale) in VIEWS {
        let mut rt = reply(zoom, scale);
        for (glyph, icon) in [("copy-glyph", &ic::OUTLINE_COPY), ("tick", &ic::CHECK)] {
            if glyph == "tick" {
                let id = rt.state.transcripts["ses_fx0004aaaaaaaaaaaaaaaaaa"].messages.last().expect("a reply").id().to_owned();
                rt.update(|s, tx| s.mark_copied(format!("copy-{id}"), tx));
                for _ in 0..30 {
                    rt.frame(1.0 / 60.0);
                }
            }
            let els = rt.elements();
            let ([bx, by], [ix, iy]) = (centre(find(&els, "copy").rect), centre(ink(icon, find(&els, glyph).rect, scale)));
            let off = [(ix - bx) * scale, (iy - by) * scale];
            assert!(off.iter().all(|d| d.abs() <= SLACK), "{glyph}'s ink at zoom {zoom}, {scale}×: {off:?} px off the square's middle");
        }
    }
}

#[test]
fn copy_lines_up_with_the_reply_and_sits_where_desktop_puts_it() {
    for (zoom, scale) in VIEWS {
        let rt = reply(zoom, scale);
        let els = rt.elements();
        let copy = find(&els, "copy");
        let glyph = ink(&ic::OUTLINE_COPY, find(&els, "copy-glyph").rect, scale);
        let last = els.iter()
            .filter(|e| !e.exiting && e.text.is_some() && e.path.contains("/timeline/t-") && e.rect[1] + e.rect[3] <= copy.rect[1] + 0.1)
            .max_by(|a, b| (a.rect[1] + a.rect[3]).total_cmp(&(b.rect[1] + b.rect[3])))
            .expect("the reply's last line");
        // Inter's letters start about half a point inside their box.
        let lead = (glyph[0] - last.rect[0]) / zoom;
        assert!((0.0..=1.0).contains(&lead), "at zoom {zoom}, {scale}×: Copy's ink starts {lead} pt from the reply's text box");
        // Desktop's 20 pt square, `margin-top: 8px` under the reply's last line: its glyph's
        // middle is 18 pt down.
        let drop = (centre(glyph)[1] - (last.rect[1] + last.rect[3])) / zoom;
        assert!((drop - 18.0).abs() * zoom * scale <= 1.0, "at zoom {zoom}, {scale}×: Copy's ink is centred {drop} pt under the last line");
    }
}
