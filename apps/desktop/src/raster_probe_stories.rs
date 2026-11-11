//! The storybook's Raster probe (Harness > Raster probe): a 1 pt black border and a 1 pt black
//! fill on white, side by side in a box keyed `raster-probe`, for checking that a capture rasters
//! at the scale it was asked for. `--fixture assets/fixture.json --scene "Harness/Raster probe"
//! --snap out.png --crop raster-probe --scale <s>` gives 16 x 16 points of each; the harness reads
//! the device pixels (`harness/rune-raster.ts`).

use rune::{Color, El, col, el, row};

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

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Harness/Raster probe", scene: |s| stage(s, view), knobs: &[], keys: &[], variants: &[] },
]);
