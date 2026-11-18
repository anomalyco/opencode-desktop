//! The storybook's raster probes (Harness):
//!
//! - **Raster probe:** a 1 pt black border and a 1 pt black fill on white, side by side in a box
//!   keyed `raster-probe`, for checking that a capture rasters at the scale it was asked for.
//!   `--fixture assets/fixture.json --scene "Harness/Raster probe" --snap out.png --crop
//!   raster-probe --scale <s>` gives 16 x 16 points of each; the harness reads the device pixels
//!   (`harness/rune-raster.ts`).
//! - **Thin lines:** black lines 0.5 to 2 pt wide at tops 20 to 20.75 pt, 8 pt wide and 4 apart, as
//!   a border (`border-top` on an empty box), a fill (a box that tall), a canvas stroke centred on
//!   the top and a canvas fill from it, in a box keyed `thin-lines` laid out as
//!   `harness/probe-thin-lines.ts` lays out its overlay on official, so the two read alike.

use rune::{Color, El, LineCap, Shape, canvas, col, el, row};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, stage};

fn view(_: &App, _: &Pal) -> El {
    let square = |key: &str| el().key(key.to_owned()).size(16.0, 16.0).fill(Color::WHITE);
    row()
        .key("raster-probe")
        .fill(Color::WHITE)
        .child(square("border").border_sides([0.0, 0.0, 0.0, 1.0], Color::BLACK))
        .child(square("rule").child(col().key("ink").w(1.0).h_full().fill(Color::BLACK)))
}

const WIDTHS: [f32; 4] = [0.5, 1.0, 1.5, 2.0];
const TOPS: [f32; 5] = [20.0, 20.25, 20.333, 20.5, 20.75];

fn thin_lines(_: &App, _: &Pal) -> El {
    let mut host = el().key("thin-lines").size(1200.0, 60.0).fill(Color::WHITE);
    let mut x = 4.0;
    for kind in 0..4 {
        for w in WIDTHS {
            for y in TOPS {
                host = host.child(match kind {
                    0 => el().absolute(x, y).w(8.0).border_sides([w, 0.0, 0.0, 0.0], Color::BLACK).border_layout(),
                    1 => el().absolute(x, y).size(8.0, w).fill(Color::BLACK),
                    2 => canvas(Shape::new().line([0.0, y], [8.0, y], w, LineCap::Butt, Color::BLACK)).absolute(x, 0.0).size(8.0, 40.0),
                    _ => canvas(Shape::new().fill(vec![[0.0, y], [8.0, y], [8.0, y + w], [0.0, y + w]], Color::BLACK)).absolute(x, 0.0).size(8.0, 40.0),
                });
                x += 12.0;
            }
        }
    }
    host
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Harness/Raster probe", scene: |s| stage(s, view), knobs: &[], keys: &[], variants: &[] },
    Story { path: "Harness/Thin lines", scene: |s| stage(s, thin_lines), knobs: &[], keys: &[], variants: &[] },
]);
