//! Long lines in the no-wrap Changes and Files viewers fade at the viewport's edge instead of
//! being cut off hard. Set `OPENCODE_DESKTOP_CODE_FADE_STILLS` to a directory to keep the stills.

use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
use rune::{Env, Runtime, col};
use rune_kit::diff::FileDiff;

use super::{CodeDocState, CodeViewer, DiffDocState, DiffViewer};
use crate::palette::pal;

const LONG: &str = "export const RECONNECT_POLICY = { base: 1000, cap: 30000, jitter: 0.2, healthyAfterMs: 60000, resetOnHealthy: true, maxAttempts: Infinity }";
const SIZE: (f32, f32) = (480.0, 160.0);
const SCALE: f32 = 3.2;

struct Host {
    diff: DiffDocState,
    file: CodeDocState,
    show_file: bool,
}

fn view(s: &Host, _cx: &mut rune::Cx<Host>) -> rune::El {
    let p = pal(true);
    let body = if s.show_file {
        CodeViewer::new("file", |h: &Host| &h.file, |h: &mut Host| &mut h.file).build(s, p)
    } else {
        DiffViewer::new("diff", |h: &Host| &h.diff, |h: &mut Host| &mut h.diff).build(s, p)
    };
    col().w_full().h_full().child(body)
}

/// Renders the viewer and returns RGBA rows; saves a still when asked.
fn render(rt: &mut Runtime<Host>, name: &str) -> (Vec<u8>, u32) {
    let gpu = Gpu::headless();
    let (wp, hp) = ((SIZE.0 * SCALE) as u32, (SIZE.1 * SCALE) as u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, wp, hp);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (wp, hp));
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, rt.clear_color());
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    let rgba = target.read(&gpu.device);
    if let Ok(dir) = std::env::var("OPENCODE_DESKTOP_CODE_FADE_STILLS") {
        write_png(&std::path::Path::new(&dir).join(name), wp, hp, &rgba).unwrap();
    }
    (rgba, wp)
}

/// The brightest text pixel in the column `x_pt` points in from the right edge, over the
/// rows of the long line (its row spans `y0..y1` points).
fn ink_at(rgba: &[u8], wp: u32, x_pt: f32, y0: f32, y1: f32) -> u32 {
    let x = (wp as f32 - x_pt * SCALE) as u32;
    ((y0 * SCALE) as u32..(y1 * SCALE) as u32)
        .map(|y| {
            let i = ((y * wp + x) * 4) as usize;
            u32::from(rgba[i]) + u32::from(rgba[i + 1]) + u32::from(rgba[i + 2])
        })
        .max()
        .unwrap_or(0)
}

fn row_of(rt: &Runtime<Host>, key: &str) -> (f32, f32) {
    let ([_, y, _, h], _) = rt.presented(key).unwrap_or_else(|| panic!("{key} drawn"));
    (y, y + h)
}

#[test]
fn long_lines_fade_at_the_right_edge_without_wrapping() {
    let text = format!("{LONG}\nshort\n");
    let mut seen = Vec::new();
    for (show_file, name, row) in [(false, "changes", "df0.n1"), (true, "files", "cl1")] {
        let mut diff = DiffDocState::new(vec![FileDiff::from_texts("src/net/policy.ts", "short\n", &text)]);
        diff.set_wrap(false);
        let mut file = CodeDocState::new("src/net/policy.ts", &text);
        file.set_wrap(false);
        let host = Host { diff, file, show_file };
        let mut rt = Runtime::new(host, view, SIZE, Env::new(true, SCALE));
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let (y0, y1) = row_of(&rt, row);
        let (rgba, wp) = render(&mut rt, &format!("{name}-long-line-zoom-1.6.png"));
        let edge = ink_at(&rgba, wp, 1.5, y0, y1);
        let inside = ink_at(&rgba, wp, 40.0, y0, y1);
        seen.push((name, edge, inside));
    }
    for (name, edge, inside) in seen {
        assert!(inside > 300, "{name}: text runs past the edge ({inside})");
        assert!(edge * 3 < inside, "{name}: the line fades at the right edge (edge {edge}, inside {inside})");
    }
}
