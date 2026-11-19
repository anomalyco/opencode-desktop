use super::*;
use super::list::Fold;
use crate::odometer::odometer;

// ---------------------------------------------------------------------------------------------
// An automation's sessions in a day group (`list.rs`): one quiet row, "Flow · 12 running ·
// 400 done", that opens to their rows. Every fold opens and shuts together, remembered.

/// The chevron's turn, as the inbox's Working header's.
const TURN: Curve = Curve::tween(0.15, Easing::cubic(0.4, 0.0, 0.2, 1.0));

/// `f`'s row in day group `day`, and its sessions' rows (`rows`) under it while open.
pub(super) fn view(p: &Pal, day: &str, f: &Fold<'_>, open: bool, rows: Vec<El>, tier: Tier) -> El {
    let key = format!("fold-{day}-{}", f.name);
    let dot = || text("·").font(UI).color(p.faint);
    let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(6.0).center_y().child(text(f.name).font(UI_MED).color(p.muted));
    if f.running > 0 {
        let label = t!("rune.home.automation.running", count = f.running);
        words = words.child(dot()).child(odometer(format!("{key}-running"), label, f.running as f64, UI, p.muted));
    }
    let done = f.total - f.running;
    if done > 0 {
        let label = t!("rune.home.automation.done", count = done);
        words = words.child(dot()).child(odometer(format!("{key}-done"), label, done as f64, UI, p.faint));
    }
    let head = row()
        .key("fold-head")
        .role("button")
        .label(f.name)
        .expanded(open)
        .focusable(true)
        .w_full()
        .h(40.0)
        .pl(if tier == Tier::Narrow { 6.0 } else { 12.0 })
        .pr(12.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, _: &mut Tx| s.toggle_home_automations())
        .child(
            el().key("chevron-box").size(16.0, 16.0).center().child(
                icon(&ic::FILL_TRIANGLE_DOWN, 14.0)
                    .key("chevron")
                    .color(p.icon_muted)
                    .group_color(p.icon)
                    .rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })
                    .rotate_motion(TURN),
            ),
        )
        .child(words);
    let mut fold = col().key(key).w_full().gap(1.0).child(head);
    if open {
        fold = fold.child(col().key("fold-rows").w_full().gap(1.0).clip_soft(0.0).children(rows));
    }
    fold
}

/// `n` sessions Flow made among the fixture's, a minute or so apart through today, the first
/// `running` of them still working: Home on a day Flow runs.
pub(crate) fn add_flow(s: &mut App, n: usize, running: usize) {
    let Some(base) = s.sessions.iter().find(|x| x.parent.is_none()).cloned() else { return };
    let tasks = ["r1-blood-b6-islands", "r1-rust-b1-logagg", "r1-blood-b5-ranges", "r1-blood-b7-tsvjoin", "envcheck-rust", "envcheck-blood-shipped"];
    for i in 0..n {
        let task = tasks[i % tasks.len()];
        let mut ses = base.clone();
        ses.id = format!("ses_flow{i:04}aaaaaaaaaaaaaaaaaa");
        ses.title = format!("flow: gauntlet R5 {task}-{:06x} {task}", 0x221c15 + i * 7919);
        ses.updated = *s.now_ms - 20_000 - i as i64 * 37_000;
        ses.created = ses.updated - 600_000;
        if i < running {
            s.running.insert(ses.id.clone());
        }
        s.sessions.push(ses);
    }
}

mod story {
    use rune::{El, Play, col};

    use crate::palette::Pal;
    use crate::stories::{Story, Trigger, replay, stage};
    use crate::{App, Route};

    const VARIANTS: &[(&str, usize)] = &[("Shut", 0), ("Open", 1)];
    const KEYS: &[Trigger] = &[
        replay(open),
        Trigger { key: 'o', label: "Open or shut the fold", run: |s, _| s.toggle_home_automations() },
        Trigger {
            key: 'u',
            label: "A lower session updates",
            run: |s, _| {
                let now = *s.now_ms;
                if let Some(ses) = s.sessions.iter_mut().filter(|x| x.parent.is_none() && !crate::view::is_automation(&x.title)).last() {
                    ses.updated = now;
                }
            },
        },
    ];

    pub(crate) fn setup(s: &mut App) {
        *s.route = Route::Home;
        super::add_flow(s, 40, 3);
        s.kernel.slots.get_mut(&crate::restore::RESTORE).inbox.home_automations = s.fixture_run.variant == 1;
    }

    fn open(s: &mut App, tx: &mut Play<App>) {
        tx.instant();
        s.sessions.retain(|x| !x.id.starts_with("ses_flow"));
        s.running.retain(|id| !id.starts_with("ses_flow"));
        s.kernel.slots.get_mut(&crate::home_search::HOME_SEARCH).hold = crate::view::HomeHold::Free;
        setup(s);
    }

    fn view(s: &App, p: &Pal) -> El {
        col()
            .key("story-home-flow")
            .w(920.0)
            .h(680.0)
            .radius(10.0)
            .fill(p.bg_base)
            .box_shadow(Pal::ring_out(p.ring))
            .clip_soft(0.0)
            .child(crate::view::home(s, p, [920.0, 680.0], [0.0, 0.0]))
    }

    rune_register::register!(crate::stories::STORY_LIST: Story = [
        Story { path: "Home/Automation fold", scene: |s| { setup(s); stage(s, view); }, knobs: &[], keys: KEYS, variants: VARIANTS },
    ]);

    rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
        ("home-flow", setup),
        ("home-flow-open", |s| {
            setup(s);
            s.kernel.slots.get_mut(&crate::restore::RESTORE).inbox.home_automations = true;
        }),
    ]);
}
