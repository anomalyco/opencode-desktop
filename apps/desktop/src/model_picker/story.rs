//! The storybook's `Pickers/Model picker`: a list with several providers, one model under three
//! of them, at the list's spacing. Its Efforts variant opens the lit model's efforts beside it.
//! The provider rail runs down its left. R replays; O opens or closes the efforts.

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

const VARIANTS: &[(&str, usize)] = &[("List", 0), ("Efforts", 1)];

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
    s.pickers.prefs = Default::default();
    s.open_picker(Kind::Model);
    s.pickers.stage_effort(CURRENT, "xhigh");
    s.choose_picker(CURRENT);
    s.composer.focused = false;
    s.open_picker(Kind::Model);
    if s.variant == 1 {
        s.open_effort_flyout(CURRENT);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::composer("Pickers/Model picker", KEYS).variants(VARIANTS),
]);
