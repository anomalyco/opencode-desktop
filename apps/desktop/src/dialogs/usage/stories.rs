//! The storybook's Usage dialog (Dialogs > Usage), with Desktop's own story data
//! (`usage/dialog.standalone.stories.tsx`): an Acme workspace with OpenCode Go, an Anthropic
//! budget and a shared pool; a per-model budget the model in use reached; the model budget or
//! the workspace's reached; out of credits; no budgets; and signed out.
//! `--playground Dialogs/Usage --variant <name> --whole-window --snap out.png`.

use rune::{El, Play, col};

use super::limits::{Budget, BudgetModel, CurrentModel, Go, Info, Meter, Workspace};
use super::{Load, Open};
use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};

const VARIANTS: &[(&str, usize)] = &[("Default", 0), ("Per model", 1), ("Model budget reached", 2), ("Workspace budget reached", 3), ("Out of credits", 4), ("No budgets", 5), ("Signed out", 6)];

const KEYS: &[Trigger] = &[replay(open)];

const HOUR: i64 = 3_600_000;

fn open(s: &mut App, _: &mut Play<App>) {
    setup(s);
}

fn setup(s: &mut App) {
    s.finish_stream();
    let resets_at = session_view::turn_errors::parse_iso("2026-11-01T00:00:00Z").unwrap_or_default();
    let model = |id: &str, name: &str, spent: Option<f64>| BudgetModel { provider: "opencode".into(), model: id.into(), name: name.into(), spent };
    let (opus, sonnet) = (|spent| model("claude-opus-5", "Claude Opus 5", spent), |spent| model("claude-sonnet-5", "Claude Sonnet 5", spent));
    let budget = |id: &str| Budget {
        id: id.into(),
        limit: 150.0,
        spent: 0.0,
        each_member: true,
        per_model: false,
        covers_all: true,
        models: Vec::new(),
        vendors: Vec::new(),
        input_price_at_least: None,
        picked: false,
        shared: false,
        resets_at,
    };
    let anthropic = |spent| Budget { covers_all: false, vendors: vec!["Anthropic".into()], models: vec![opus(None), sonnet(None)], spent, ..budget("anthropic") };
    let pool = |spent| Budget { each_member: false, shared: true, limit: 1_000.0, spent, ..budget("pool") };
    let now = *s.now_ms;
    let meter = |percent, hours| Meter { percent, limited: false, resets_at: now + hours * HOUR };
    let info = |budgets: Vec<Budget>, go: bool| Info {
        workspace: Workspace { name: Some("Acme".into()), usage_url: "https://opencode.ai/wrk_1/activity/overview".into() },
        go: go.then(|| Go { rolling: meter(42.0, 3), weekly: meter(18.0, 100), monthly: meter(7.0, 500) }),
        budgets,
    };
    let current = Some(CurrentModel { provider: "opencode".into(), model: "claude-opus-5".into(), name: "Claude Opus 5".into() });
    let per_model = Budget {
        covers_all: false,
        per_model: true,
        limit: 20.0,
        spent: 20.0,
        picked: true,
        models: vec![opus(Some(20.0)), sonnet(Some(4.5))],
        ..budget("each")
    };
    let (load, model, credits) = match *s.variant {
        1 => (Load::Ready(info(vec![per_model], false)), current, false),
        2 => (Load::Ready(info(vec![anthropic(150.42), pool(394.57)], false)), current, false),
        3 => (Load::Ready(info(vec![anthropic(80.0), pool(1_000.12)], false)), None, false),
        4 => (Load::Ready(info(vec![anthropic(80.0), pool(394.57)], false)), None, true),
        5 => (Load::Ready(info(Vec::new(), false)), None, false),
        6 => (Load::SignedOut, None, false),
        _ => (Load::Ready(info(vec![anthropic(123.14), pool(394.57)], true)), None, false),
    };
    s.usage.open = Some(Open { directory: s.composer_directory(), model, credits, load, issued: Default::default() });
}

/// The dialog alone, centred on the stage over the app's scrim.
fn view(s: &App, p: &Pal) -> El {
    col().key("story-usage").w_full().h(640.0).center().fill(p.bg_base).children(super::view::dialog(s, p, [1280.0, 640.0]))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Dialogs/Usage", scene: |s| stage(s, view), knobs: &[], keys: KEYS, variants: VARIANTS },
]);

// `--scene usage`: the Default dialog over the app, to see it in a short window, where its
// content scrolls.
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("usage", setup),
]);
