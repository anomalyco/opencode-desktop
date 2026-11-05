//! Desktop's model search, and the typo that still finds its model (t1389).
use super::search::{normalize, score};

fn gemini() -> [&'static str; 3] {
    ["Gemini 4 Argon", "google", "gemini-4-argon"]
}

fn opus() -> [&'static str; 3] {
    ["Claude Opus 5.5", "anthropic", "claude-opus-5-5"]
}

#[test]
fn punctuation_and_case_dont_matter() {
    assert_eq!(normalize("GPT-5.5"), "gpt 5 5");
    assert_eq!(normalize("  open_ai//"), "open ai");
    assert!(score("gpt 5", &["GPT-5.5", "openai", "gpt-5-5"]).is_some(), "Desktop's own example");
    assert!(score("gpt5", &["GPT-5.5", "openai", "gpt-5-5"]).is_some(), "the spaces come out");
    assert!(score("open ai", &["GPT-5.5", "OpenAI", "gpt-5-5"]).is_some(), "the provider counts");
    assert!(score("claude", &["GPT-5.5", "openai", "gpt-5-5"]).is_none(), "and nothing else does");
}

#[test]
fn the_id_finds_a_model_whose_name_the_query_misses() {
    assert!(score("opus55", &opus()).is_some(), "claude-opus-5-5 in the id");
    assert!(score("gemini4", &gemini()).is_some(), "the name and the id agree");
}

#[test]
fn a_typo_finds_the_model_it_meant() {
    assert!(score("gemiin", &gemini()).is_some(), "\"gemiin\" reaches Gemini");
    assert!(score("gemni", &gemini()).is_some(), "a letter dropped reaches it too");
    assert!(score("gmi", &gemini()).is_some(), "and a sketch of the name");
    assert!(score("gemiin", &opus()).is_none(), "without reaching a model it doesn't fit");
    assert!(score("xyzzy", &gemini()).is_none(), "and nonsense finds nothing");
}

#[test]
fn what_desktop_matches_ranks_above_a_typo() {
    let typo = score("gemiin", &gemini()).unwrap();
    let prefix = score("gemini", &gemini()).unwrap();
    let inside = score("argon", &gemini()).unwrap();
    assert!(prefix > inside, "a name that starts with the query leads one that contains it");
    assert!(inside > typo, "and both lead a word only the letters in order find: {inside} vs {typo}");
}

#[test]
fn the_name_ranks_above_the_id_for_the_same_match() {
    let named = score("argon", &["Gemini 4 Argon", "google", "gemini-4-bo"]).unwrap();
    let identified = score("argon", &["Gemini 4 Bo", "google", "gemini-4-argon"]).unwrap();
    assert!(named > identified, "a hit in the name beats the same hit in the id: {named} vs {identified}");
}

#[test]
fn an_empty_query_matches_everything() {
    assert_eq!(score("", &gemini()), Some(0));
    assert_eq!(score("  ", &gemini()), Some(0));
    assert_eq!(score("...", &gemini()), Some(0));
}
