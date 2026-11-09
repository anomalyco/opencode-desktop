use super::*;

fn hit(query: &str, name: &str) -> Option<Hit> {
    score(&Needle::new(query), &Prepared::new(name))
}

fn tier(query: &str, name: &str) -> Option<Tier> {
    hit(query, name).map(|h| h.tier)
}

#[test]
fn spaces_case_and_punctuation_dont_matter() {
    // Kit's Scrawl (Nov 8): `@runedesktop` found unrelated files, not the session.
    for q in ["runedesktop", "runedes", "rune des", "RUNE-DESK", "Rune Desktop"] {
        assert!(tier(q, "RUNE DESKTOP") >= Some(Tier::Prefix), "{q}");
    }
    assert_eq!(tier("rune desktop", "RUNE DESKTOP"), Some(Tier::Exact));
    assert_eq!(tier("gpt5", "GPT-5.5"), Some(Tier::Prefix));
}

#[test]
fn tiers_rank_whole_then_start_then_word_then_inside_then_pieces() {
    assert_eq!(tier("rune", "RUNE"), Some(Tier::Exact));
    assert_eq!(tier("rune", "rune-ui"), Some(Tier::Prefix));
    assert_eq!(tier("desk", "RUNE DESKTOP"), Some(Tier::Word));
    assert_eq!(tier("connect", "Fix flaky reconnect"), Some(Tier::Inside));
    assert_eq!(tier("fla rec", "Fix flaky reconnect"), Some(Tier::Scattered));
    assert_eq!(tier("seshist", "sessionHistory"), Some(Tier::Scattered), "a case change starts a word");
    assert!(hit("rune", "RUNE") > hit("rune", "rune-ui"));
    assert!(hit("rune", "rune-ui") > hit("rune", "tools/rune-status"));
    assert!(hit("desk", "RUNE DESKTOP") > hit("desk", "the bigdesk"));
    assert!(hit("connect", "reconnect") > hit("flarec", "Fix flaky reconnect"));
}

#[test]
fn scattered_letters_are_not_a_match() {
    // What `/api/fs/find` answered for `@runedes` in the Scrawl.
    assert_eq!(tier("runedes", "research/composer-docks/runs/audit3/before-audit/probe.json"), None);
    assert_eq!(tier("runedes", "research/mac-app-inspiration/thumbs/images/opencode/opencode-01-onboarding-first-run.webp"), None);
    assert_eq!(tier("xyz", "@explore"), None);
    assert_eq!(tier("xlo", "@explore"), None, "a lone letter inside a word isn't a piece");
    assert_eq!(tier("epl", "@explore"), Some(Tier::Scattered));
    assert_eq!(tier("exr", "@explore"), None);
    assert_eq!(tier("exlo", "@explore"), Some(Tier::Scattered));
}

#[test]
fn files_prefer_their_own_name_and_shorter_paths() {
    let own = hit("stream", "src/ui/stream.rs");
    let folder = hit("stream", "src/stream/reconnect.rs");
    assert!(own > folder, "{own:?} {folder:?}");
    assert!(hit("mod", "src/mod.rs") > hit("mod", "src/deeply/nested/folder/mod.rs"));
}

#[test]
fn narrowing_follows_the_typed_prefix() {
    assert!(Needle::new("rune d").extends(&Needle::new("rune")));
    assert!(!Needle::new("run").extends(&Needle::new("rune")));
}
