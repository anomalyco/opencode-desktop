//! A model's group header says its provider's name (t1389): `/api/model` names a provider by its
//! id (`console-openai-test`), `/api/provider` names it as a person reads it ("Anomaly / RITB").
use serde_json::json;

use super::Pickers;

fn models() -> serde_json::Value {
    json!([
        { "id": "gpt-6-astra", "providerID": "console-openai-test", "name": "GPT-6 Astra" },
        { "id": "opus", "providerID": "unknown-provider", "name": "Opus" },
    ])
}

fn providers() -> serde_json::Value {
    json!([{ "id": "console-openai-test", "name": "Anomaly / RITB" }])
}

#[test]
fn a_group_header_says_the_providers_name() {
    let mut pk = Pickers::default();
    pk.load_live(&models(), &json!([]));
    assert_eq!(pk.models[0].group, "console-openai-test", "the model list names a provider by id only");
    pk.load_providers(&providers());
    assert_eq!(pk.models[0].group, "Anomaly / RITB", "the header says the name");
    assert_eq!(pk.models[1].group, "unknown-provider", "a provider the list leaves out keeps its id");
    assert_eq!(pk.models[0].provider, "console-openai-test", "the id still keys the rail and its stops");
}

#[test]
fn names_loaded_before_the_models_show_too() {
    let mut pk = Pickers::default();
    pk.load_providers(&providers());
    pk.load_live(&models(), &json!([]));
    assert_eq!(pk.models[0].group, "Anomaly / RITB");
    assert_eq!(pk.models[1].group, "unknown-provider");
}

#[test]
fn a_name_that_only_repeats_the_id_is_no_name() {
    let mut pk = Pickers::default();
    pk.load_providers(&json!([
        { "id": "console-openai", "name": "console-openai" },
        { "id": "google", "name": "" },
        { "id": "openai" },
    ]));
    assert!(pk.providers.is_empty(), "nothing to say: {:?}", pk.providers);
}

#[test]
fn a_fixture_may_name_its_providers_too() {
    let mut pk = Pickers::default();
    pk.load(&json!({
        "models": [{ "id": "gpt-6-astra", "provider": "console-openai-test", "group": "console-openai-test", "name": "GPT-6 Astra" }],
        "providers": [{ "id": "console-openai-test", "name": "Anomaly / RITB" }],
    }));
    assert_eq!(pk.models[0].group, "Anomaly / RITB");
    let mut plain = Pickers::default();
    plain.load(&json!({ "models": [{ "id": "m", "provider": "x", "group": "X", "name": "M" }] }));
    assert_eq!(plain.models[0].group, "X", "a fixture with no providers keeps its own group names");
}
