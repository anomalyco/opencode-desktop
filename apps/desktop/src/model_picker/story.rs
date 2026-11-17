//! The storybook's `Pickers/Model picker`: Desktop's model dialog with several providers, one
//! model under three of them, and three recent models under Recent. Its group headers say the
//! providers' names ("Anomaly / RITB"), as the live list's do. Its Efforts variant opens the lit
//! model's flyout beside it; its Favorite variant stars one model, so Favorites leads the list.
//! R replays; O opens or closes the flyout.

use crate::pickers::{EffortFlyout as _, PickerHost as _};
use rune::Play;

use crate::App;
use crate::pickers::Kind;
use crate::stories::{Story, Trigger, replay};

const CURRENT: &str = "opencode:claude-opus-5-5";

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'o', label: "Open or close the efforts", run: |s, _| {
        if let Some(key) = s.picker_active() { s.toggle_effort_flyout(&key); }
    } },
];

const VARIANTS: &[(&str, usize)] = &[("List", 0), ("Efforts", 1), ("Favorite", 2), ("Keyboard budget", 3), ("Manage", 4)];

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let efforts = serde_json::json!([{"id": "low"}, {"id": "medium"}, {"id": "high"}, {"id": "xhigh"}]);
    let model = |id: &str, provider: &str, name: &str| serde_json::json!({"id": id, "providerID": provider, "name": name, "variants": efforts, "cost": [{"input": 3.0}]});
    let models = serde_json::json!([
        model("claude-opus-5-5", "opencode", "Claude Opus 5.5"),
        model("claude-sonnet-5-5", "opencode", "Claude Sonnet 5.5"),
        model("gpt-6.1-terra", "github-copilot", "GPT-6.1 Terra"),
        model("gpt-6.1-terra", "openai", "GPT-6.1 Terra"),
        model("gpt-6-astra", "openai", "GPT-6 Astra"),
        model("gpt-6.1-terra", "console-openai-test", "GPT-6.1 Terra"),
        model("kimi-k3.1-pro", "console-moonshot-ai", "Kimi K3.1 Pro"),
        model("gemini-4-carbon", "console-google", "Gemini 4 Carbon"),
        model("grok-5", "xai", "Grok 5"),
    ]);
    s.pickers.load_live(&models, &serde_json::json!([{"id": "build"}]));
    // What the group headers say: `/api/provider`'s names, not the ids `/api/model` gives.
    s.pickers.load_providers(&serde_json::json!([
        { "id": "opencode", "name": "Anomaly / OpenCode" },
        { "id": "github-copilot", "name": "GitHub Copilot" },
        { "id": "openai", "name": "OpenAI" },
        { "id": "console-openai-test", "name": "Anomaly / RITB" },
        { "id": "console-moonshot-ai", "name": "Anomaly / Moonshot AI" },
        { "id": "console-google", "name": "Anomaly / Google" },
        { "id": "xai", "name": "xAI" },
    ]));
    s.pickers.prefs = Default::default();
    s.open_picker(Kind::Model);
    s.pickers.stage_effort(CURRENT, "xhigh");
    s.choose_picker(CURRENT);
    s.pickers.prefs.recent = ["console-openai-test:gpt-6.1-terra", "openai:gpt-6-astra", "xai:grok-5"].map(str::to_owned).to_vec();
    if s.variant == 2 {
        s.pickers.prefs.favorites = vec!["openai:gpt-6-astra".to_owned()];
    }
    s.composer.focused = false;
    if s.variant == 3 {
        s.fixture["usage_limits"] = serde_json::json!({"workspace":{"name":"Acme","usageURL":"https://example.com/usage"},
            "budgets":[{"id":"team", "limit":100, "spent":28, "coverage":"all", "resetsAt":1796083200000_i64}]});
    }
    s.open_picker(Kind::Model);
    if s.variant == 1 {
        s.toggle_effort_flyout(CURRENT);
    }
    if s.variant == 3 {
        s.pickers.activate(CURRENT.into());
        s.picker_key(&rune::KeyEvent { key: rune::Key::Right, mods: Default::default() }, &mut rune::Tx::default());
    }
    if s.variant == 4 {
        s.close_picker(false);
        s.pickers.open_manage();
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::composer("Pickers/Model picker", KEYS).variants(VARIANTS),
]);
