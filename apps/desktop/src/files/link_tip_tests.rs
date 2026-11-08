use super::link_tip::label;

const DIR: &str = "/home/alex/proj";
const HOME: &str = "/home/alex";

fn tip(href: &str) -> Option<String> {
    label(href, "", DIR, HOME, Some(&["Makefile".to_owned()]))
}

#[test]
fn web_links_show_host_and_path() {
    assert_eq!(tip("https://www.example.com/docs/a/?q=1#top").as_deref(), Some("www.example.com/docs/a"));
    assert_eq!(tip("http://example.com/").as_deref(), Some("example.com"));
    assert_eq!(tip("mailto:alex@example.com").as_deref(), Some("mailto:alex@example.com"));
}

#[test]
fn files_show_their_path_in_the_session_folder() {
    assert_eq!(tip("src/main.rs#L12-L20").as_deref(), Some("src/main.rs:12-20"));
    assert_eq!(tip("/home/alex/proj/docs/plan.md").as_deref(), Some("docs/plan.md"));
    assert_eq!(tip("./README.md:7").as_deref(), Some("README.md:7"));
    assert_eq!(tip("/home/alex/other/x.md").as_deref(), Some("~/other/x.md"));
    assert_eq!(tip("/etc/hosts").as_deref(), Some("/etc/hosts"));
    assert_eq!(label("../b.md", "docs/a", DIR, HOME, None).as_deref(), Some("docs/b.md"));
}

#[test]
fn a_bare_word_that_isnt_a_file_points_nowhere() {
    assert_eq!(tip("link").as_deref(), Some(opencode_i18n::t!("rune.files.link.nowhere")));
    assert_eq!(tip("Makefile").as_deref(), Some("Makefile"));
    assert_eq!(label("link", "", DIR, HOME, None).as_deref(), Some("link"), "before the file list loads, a click decides");
}
