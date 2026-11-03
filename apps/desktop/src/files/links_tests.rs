use super::links::{Link, classify, slug};

const DIR: &str = "/Users/demo/code/lumen";
const HOME: &str = "/Users/demo";

fn reply(href: &str) -> Link {
    classify(href, "", DIR, HOME)
}

fn inside(path: &str, lines: Option<(u32, u32)>, heading: Option<&str>) -> Link {
    Link::Inside { path: path.into(), lines, heading: heading.map(Into::into) }
}

fn outside(path: &str) -> Link {
    Link::Outside { path: path.into() }
}

#[test]
fn web_and_app_links_stay_as_written() {
    for href in [
        "https://example.com/a?b=1#c",
        "http://localhost:3000",
        "mailto:alex@example.com",
        "seer://item/t12",
        "slack://channel?id=C1",
        "vscode://file/x",
    ] {
        assert_eq!(reply(href), Link::Web(href.into()), "{href}");
    }
}

#[test]
fn relative_paths_resolve_against_the_session_folder() {
    assert_eq!(reply("docs/proposal.md"), inside("docs/proposal.md", None, None));
    assert_eq!(reply("./docs/proposal.md"), inside("docs/proposal.md", None, None));
    assert_eq!(reply("README.md"), inside("README.md", None, None));
}

#[test]
fn absolute_paths_inside_the_folder_open_relative_and_others_go_outside() {
    assert_eq!(reply("/Users/demo/code/lumen/src/net/stream.ts"), inside("src/net/stream.ts", None, None));
    assert_eq!(reply("/Users/demo/code/harbor/README.md"), outside("/Users/demo/code/harbor/README.md"));
    // A sibling folder sharing the prefix isn't inside.
    assert_eq!(reply("/Users/demo/code/lumen-old/a.md"), outside("/Users/demo/code/lumen-old/a.md"));
    assert_eq!(reply("../harbor/a.md"), outside("/Users/demo/code/harbor/a.md"));
}

#[test]
fn home_and_file_urls_expand() {
    assert_eq!(reply("~/code/lumen/Cargo.toml"), inside("Cargo.toml", None, None));
    assert_eq!(reply("~/notes.md"), outside("/Users/demo/notes.md"));
    assert_eq!(reply("file:///Users/demo/code/lumen/docs/proposal.md"), inside("docs/proposal.md", None, None));
    assert_eq!(reply("file://localhost/Users/demo/notes.md"), outside("/Users/demo/notes.md"));
    assert_eq!(reply("FILE:///tmp/x.txt"), outside("/tmp/x.txt"));
}

#[test]
fn line_references_are_kept() {
    assert_eq!(reply("src/net/stream.ts#L12"), inside("src/net/stream.ts", Some((12, 12)), None));
    assert_eq!(reply("src/net/stream.ts#L12-L20"), inside("src/net/stream.ts", Some((12, 20)), None));
    assert_eq!(reply("src/net/stream.ts#L12-20"), inside("src/net/stream.ts", Some((12, 20)), None));
    assert_eq!(reply("src/net/stream.ts:12"), inside("src/net/stream.ts", Some((12, 12)), None));
    assert_eq!(reply("src/net/stream.ts:12-20"), inside("src/net/stream.ts", Some((12, 20)), None));
    assert_eq!(reply("src/net/stream.ts:12:5"), inside("src/net/stream.ts", Some((12, 12)), None));
    assert_eq!(reply("/Users/demo/code/lumen/src/a.rs:7"), inside("src/a.rs", Some((7, 7)), None));
    assert_eq!(reply("file:///Users/demo/code/lumen/src/a.rs#L3"), inside("src/a.rs", Some((3, 3)), None));
}

#[test]
fn percent_encoding_is_decoded() {
    assert_eq!(reply("docs/design%20notes.md"), inside("docs/design notes.md", None, None));
    assert_eq!(reply("file:///Users/demo/code/lumen/docs/caf%C3%A9.md"), inside("docs/café.md", None, None));
    // A stray `%` stays.
    assert_eq!(reply("docs/100%.md"), inside("docs/100%.md", None, None));
}

#[test]
fn headings_are_kept_apart_from_lines() {
    assert_eq!(reply("docs/proposal.md#plan"), inside("docs/proposal.md", None, Some("plan")));
    assert_eq!(reply("#where-titles-show"), Link::Heading("where-titles-show".into()));
    assert_eq!(slug("Where titles show"), "where-titles-show");
    assert_eq!(slug("Why `titles` go stale!"), "why-titles-go-stale");
}

#[test]
fn links_in_a_file_resolve_against_its_folder() {
    let from_docs = |href| classify(href, "docs", DIR, HOME);
    assert_eq!(from_docs("first-principles.md"), inside("docs/first-principles.md", None, None));
    assert_eq!(from_docs("./first-principles.md#the-rule"), inside("docs/first-principles.md", None, Some("the-rule")));
    assert_eq!(from_docs("../src/net/stream.ts#L4"), inside("src/net/stream.ts", Some((4, 4)), None));
    assert_eq!(from_docs("../../harbor/README.md"), outside("/Users/demo/code/harbor/README.md"));
    assert_eq!(from_docs("/Users/demo/code/lumen/Cargo.toml"), inside("Cargo.toml", None, None));
}

#[test]
fn without_a_folder_relative_paths_stay_relative() {
    assert_eq!(classify("docs/a.md", "", "", HOME), inside("docs/a.md", None, None));
    assert_eq!(classify("/tmp/a.md", "", "", HOME), outside("/tmp/a.md"));
    assert_eq!(classify("../a.md", "", "", HOME), outside("../a.md"));
}

#[test]
fn bare_words_point_nowhere_and_paths_and_urls_do_not() {
    for href in ["link", "url", "TODO", "", "  "] {
        assert!(matches!(reply(href), Link::Dead(_)), "{href:?}");
    }
    for href in ["a.md", "docs/plan", "./x", "~/x", "#top", "https://example.com", "seer:item", "mailto:a@b.co", "x.md:3"] {
        assert!(!matches!(reply(href), Link::Dead(_)), "{href}");
    }
}
