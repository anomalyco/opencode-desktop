//! The storybook's Rewind story (Dev › Rewind): a card slides in and, just after it settles, blips
//! 18 pt down for one frame and back, the kind of glitch nobody can point at once it's gone. The
//! window then freezes on that frame, with the card outlined and the jump red on the track.
//! R replays; F freezes again.

use rune::{El, Play, col, el, text};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Trigger, replay};
use crate::view::{UI, UI_MED, UI_SMALL};

const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'f', label: "Freeze", run: |s, _| freeze_on_jump(s) }];

/// Where the card rests, and how far the blip throws it.
const REST: f32 = 180.0;
const BLIP: f32 = 18.0;

fn open(s: &mut App, p: &mut Play<App>) {
    s.diag.glitch.close_scrubber();
    s.settings.record_glitches = true;
    s.diag.glitch.demo_y = 40.0;
    crate::stories::watch(s, stage);
    p.after(0.15, |s, p| {
        s.diag.glitch.demo_y = REST;
        p.motion(rune::Spring::new(0.4, 0.0));
    });
    p.after(0.9, |s, p| {
        s.diag.glitch.demo_y += BLIP;
        p.instant();
    });
    p.after(0.9 + 1.0 / 60.0, |s, p| {
        s.diag.glitch.demo_y -= BLIP;
        p.instant();
    });
    p.after(1.4, |s, _| freeze_on_jump(s));
    // Steps back over the jump a frame at a time, then selects the stretch after it again.
    for (i, d) in [-3i32, -2, -1, 0, 1, 2].into_iter().enumerate() {
        p.after(2.0 + 0.3 * i as f64, move |s, _| step_to(s, d));
    }
    p.after(4.0, |s, _| select_jump(s));
}

/// Shows the frame `d` frames from the first jump.
fn step_to(s: &mut App, d: i32) {
    let Some(scrub) = s.diag.glitch.scrub_mut() else { return };
    let Some(clip) = scrub.clip().cloned() else { return };
    let Some(k) = (0..clip.len()).find(|&k| clip.has_jump(k)) else { return };
    scrub.seek((k as i32 + d).max(0) as usize, false);
}

/// Freezes the window on the first jump, with the stretch after it selected.
fn freeze_on_jump(s: &mut App) {
    super::freeze(s);
    select_jump(s);
}

/// Selects the eight frames after the first jump, with Kit's note.
fn select_jump(s: &mut App) {
    let Some(scrub) = s.diag.glitch.scrub_mut() else { return };
    let Some(clip) = scrub.clip().cloned() else { return };
    let Some(k) = (0..clip.len()).find(|&k| clip.has_jump(k)) else { return };
    scrub.seek(k + 8, false);
    scrub.seek(k, true);
    scrub.set_note("The Working card blips down a line right after it lands");
}

fn stage(s: &App, p: &Pal) -> El {
    let card = col()
        .key("glitch-demo-card")
        .absolute(48.0, s.diag.glitch.demo_y)
        .w(360.0)
        .gap(6.0)
        .padding(14.0)
        .radius(10.0)
        .fill(p.layer1)
        .border(0.5, p.border_muted)
        .child(text("Working").font(UI_MED).color(p.text))
        .child(text("Reading crates/rune/src/rewind/clip.rs").font(UI_SMALL).color(p.muted));
    el().key("glitch-demo").w_full().h_full().fill(p.bg_base)
        .child(text("Assistant").key("glitch-demo-title").absolute(48.0, 24.0).font(UI).color(p.faint))
        .child(card)
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Dev/Rewind", scene: |s| crate::stories::watch(s, stage), knobs: &[], keys: KEYS, variants: &[] },
]);
