//! Desktop host adapter for `opencode-feature-usage` (`features/usage`).

pub use opencode_feature_usage::*;

use opencode_kernel::{Feature, KernelBuilder, OverlayEntry, OverlayEvent, OverlayKind};
use rune::Tx;
use serde_json::Value;

use crate::App;
use crate::pickers::PickerHost as _;

impl UsageHost for App {
    fn usage(&self) -> &Usage {
        &self.usage
    }

    fn usage_mut(&mut self) -> &mut Usage {
        &mut self.usage
    }

    fn composer_directory(&self) -> String {
        App::composer_directory(self)
    }

    fn selected_model(&self) -> Option<CurrentModel> {
        let selection = self.selection();
        self.pickers()
            .models
            .iter()
            .find(|m| m.provider == selection.model.provider && m.id == selection.model.id)
            .map(|m| CurrentModel {
                provider: m.provider.clone(),
                model: m.id.clone(),
                name: m.name.clone(),
            })
    }

    fn is_live(&self) -> bool {
        self.live()
    }

    fn fixture_usage_limits(&self) -> Option<&Value> {
        self.fixture.get("usage_limits")
    }

    fn fetch_usage_limits(&mut self, directory: String, seq: u32) {
        self.job_after(std::time::Duration::from_secs_f64(TIMEOUT_S), move |_| {
            Box::new(move |s: &mut App, tx: &mut Tx| timed_out(s, seq, tx)) as crate::jobs::Apply
        });
        self.job(move |srv| {
            let path = limits_endpoint(&directory);
            let answer = srv.get(&path).map_err(|e| e.to_string());
            Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                apply_response(s, seq, &directory, answer, tx);
            })
        });
    }

    fn fetch_picker_usage_limits(&mut self, directory: String) {
        self.job(move |srv| {
            let path = limits_endpoint(&directory);
            let answer = srv.get(&path).map_err(|e| e.to_string());
            Box::new(move |s: &mut App, tx: &mut Tx| {
                apply_picker_response(s, &directory, answer, tx);
            })
        });
    }

    fn out_of_credits(&self) -> bool {
        self.active_session_id()
            .and_then(|id| self.transcripts.get(id))
            .is_some_and(transcript_out_of_credits)
    }

    fn now_ms(&self) -> i64 {
        *self.now_ms
    }

    fn zoom(&self) -> f32 {
        *self.zoom
    }

    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        crate::open_in::open_external(tx, url);
    }

    fn open_model_picker(&mut self, _tx: &mut Tx) {
        self.open_picker(crate::pickers::Kind::Model);
    }

    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>) {
        let mut entry = OverlayEntry::new(id, OverlayKind::Dialog);
        if let Some(focus) = return_focus {
            entry = entry.with_return_focus(focus);
        }
        self.kernel.step_overlay(OverlayEvent::Push(entry));
    }

    fn on_overlay_closed(&mut self, id: &'static str) {
        self.kernel.step_overlay(OverlayEvent::Close(id));
    }
}

impl App {
    pub(crate) fn refresh_picker_usage(&mut self) {
        refresh_picker(self);
    }

    pub(crate) fn picker_budget_label(&self, key: &str) -> Option<(String, bool)> {
        let model = self.pickers.models.iter().find(|m| m.key == key && !m.free)?;
        let served = model.provider != "opencode-go"
            && (model.provider == "opencode"
                || self
                    .provider_infos(None)
                    .unwrap_or_default()
                    .iter()
                    .any(|p| p.id == model.provider && p.integration.as_deref() == Some("opencode")));
        warning_for(self, &model.provider, &model.id, served)
    }

    /// `usage.view`: opens the dialog over the composer's location and reads its limits.
    pub fn open_usage(&mut self, tx: &mut Tx) {
        open(self, tx);
    }

    pub fn close_usage(&mut self) {
        close(self);
    }
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("usage.view", opencode_i18n::key!("command.usage.view"), |s, tx| s.open_usage(tx))
        .describe(opencode_i18n::key!("command.usage.view.description"))
        .when(App::has_composer),
]);

/// Kernel feature registration for the Usage dialog.
pub struct UsageFeature;

impl Feature<App> for UsageFeature {
    fn id(&self) -> &'static str {
        "feature.usage"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_overlay_dismiss(OVERLAY_ID, |s, tx| {
            tx.instant();
            s.close_usage();
        });
    }
}

mod stories {
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

    const VARIANTS: &[(&str, usize)] = &[
        ("Default", 0),
        ("Per model", 1),
        ("Model budget reached", 2),
        ("Workspace budget reached", 3),
        ("Out of credits", 4),
        ("No budgets", 5),
        ("Signed out", 6),
    ];

    const KEYS: &[Trigger] = &[replay(open)];

    const HOUR: i64 = 3_600_000;

    fn open(s: &mut App, _: &mut Play<App>) {
        setup(s);
    }

    fn setup(s: &mut App) {
        s.finish_stream();
        let resets_at = session_view::turn_errors::parse_iso("2026-11-01T00:00:00Z").unwrap_or_default();
        let model = |id: &str, name: &str, spent: Option<f64>| BudgetModel {
            provider: "opencode".into(),
            model: id.into(),
            name: name.into(),
            spent,
        };
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
        let anthropic = |spent| Budget {
            covers_all: false,
            vendors: vec!["Anthropic".into()],
            models: vec![opus(None), sonnet(None)],
            spent,
            ..budget("anthropic")
        };
        let pool = |spent| Budget { each_member: false, shared: true, limit: 1_000.0, spent, ..budget("pool") };
        let now = *s.now_ms;
        let meter = |percent, hours| Meter { percent, limited: false, resets_at: now + hours * HOUR };
        let info = |budgets: Vec<Budget>, go: bool| Info {
            workspace: Workspace {
                name: Some("Acme".into()),
                usage_url: "https://opencode.ai/wrk_1/activity/overview".into(),
            },
            go: go.then(|| Go {
                rolling: meter(42.0, 3),
                weekly: meter(18.0, 100),
                monthly: meter(7.0, 500),
            }),
            budgets,
        };
        let current = Some(CurrentModel {
            provider: "opencode".into(),
            model: "claude-opus-5".into(),
            name: "Claude Opus 5".into(),
        });
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
        s.usage.open = Some(Open {
            directory: s.composer_directory(),
            model,
            credits,
            load,
            issued: Default::default(),
        });
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
}
