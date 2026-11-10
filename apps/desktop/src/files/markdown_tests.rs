use rune::{Env, Runtime, TextEdit};

use super::markdown::{WrapPrefs, is_markdown, is_prose, shows_rendered};
use crate::fixture_ids::THIRD;
use crate::{App, FIXTURE, Route, spec};

const PROPOSAL: &str = "docs/proposal.md";
const PRINCIPLES: &str = "docs/first-principles.md";

/// The fixture on the session whose reply links `docs/proposal.md`.
fn runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        a.route = Route::here(THIRD.into());
        a.finish_stream();
        crate::plugin_host::setup(&mut a, std::sync::Arc::new(|| {}));
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, (1440.0, 900.0), Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

fn click(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} on screen"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

fn active_file(rt: &Runtime<App>) -> Option<(String, Option<String>)> {
    rt.state.active_file_tab()
}

fn open(rt: &mut Runtime<App>, path: &'static str) {
    rt.update(|s, tx| s.open_workspace_file(path, false, None, tx));
    settle(rt);
}

/// The file viewer wraps code too until it's turned off, as Desktop's (826dfb5a); prose and code
/// each keep their own choice.
#[test]
fn prose_and_code_wrap_until_turned_off() {
    let w = WrapPrefs::default();
    for path in ["docs/a.md", "A.MARKDOWN", "page.mdx", "notes.txt", "guide.rst", "book.adoc", "LICENSE", "README", "sub/LICENSE-MIT", "COPYING"] {
        assert!(w.wraps(path), "{path} wraps");
        assert!(is_prose(path));
    }
    for path in ["src/main.rs", "src/net/stream.ts", "Cargo.toml", "Makefile", ".gitignore", "Dockerfile", "LICENSES.json"] {
        assert!(w.wraps(path), "{path} wraps");
        assert!(!is_prose(path));
    }
    let flipped = WrapPrefs { prose: Some(true), code: Some(false) };
    assert!(flipped.wraps("docs/a.md") && !flipped.wraps("src/main.rs"));
    assert!(is_markdown("docs/a.md") && is_markdown("x.MDX") && !is_markdown("notes.txt"));
}

#[test]
fn opened_files_wrap_by_kind_and_a_flip_carries_to_the_kind() {
    let mut rt = runtime();
    open(&mut rt, PROPOSAL);
    open(&mut rt, "src/net/stream.ts");
    let docs = |rt: &Runtime<App>, p: &str| rt.state.files.sessions[THIRD].docs[p].wrap();
    assert!(docs(&rt, PROPOSAL));
    assert!(docs(&rt, "src/net/stream.ts"));

    // Kit turns wrap off for one code file: every code file stops wrapping, prose untouched.
    rt.update(|s, _| s.set_file_wrap(THIRD, "src/net/stream.ts", false));
    settle(&mut rt);
    assert!(!docs(&rt, "src/net/stream.ts") && !docs(&rt, "src/net/backoff.ts"));
    assert_eq!(rt.state.files.wrap, WrapPrefs { prose: None, code: Some(false) });
    open(&mut rt, "src/sidebar/store.ts");
    assert!(!docs(&rt, "src/sidebar/store.ts"));
    assert!(docs(&rt, PROPOSAL));
}

#[test]
fn a_file_link_in_a_reply_opens_the_file_beside_it() {
    let mut rt = runtime();
    assert!(active_file(&rt).is_none());
    click(&mut rt, PROPOSAL);
    assert_eq!(active_file(&rt), Some((THIRD.to_owned(), Some(PROPOSAL.to_owned()))));
    assert!(rt.state.side_open());
    assert!(shows_rendered(&rt.state.files.sessions[THIRD], PROPOSAL));
    assert!(rt.find(&super::markdown::scroll_key(THIRD, PROPOSAL)).is_some(), "rendered page shows");
    // The tree marks what the session changed, as it does from ⌘P.
    assert!(rt.state.review.sessions.get(THIRD).is_some_and(|r| r.load.loaded_revision().is_some()));
    // The reply keeps focus: the link opened the file, it didn't pull Kit into it.
    assert!(!rt.state.side.focused);
}

#[test]
fn a_link_in_a_rendered_file_resolves_against_its_folder() {
    let mut rt = runtime();
    open(&mut rt, PROPOSAL);
    // `[first principles](first-principles.md)` in `docs/proposal.md`.
    click(&mut rt, "first-principles.md");
    assert_eq!(active_file(&rt), Some((THIRD.to_owned(), Some(PRINCIPLES.to_owned()))));
}

#[test]
fn line_links_select_lines_and_missing_files_say_so() {
    let mut rt = runtime();
    rt.update(|s, tx| s.open_reply_link("src/net/stream.ts#L3-L5", tx));
    settle(&mut rt);
    let doc = &rt.state.files.sessions[THIRD].docs["src/net/stream.ts"];
    assert_eq!(doc.selection().map(|s| s.ordered()), Some((3, 5)));

    // A line in a Markdown file is in its source.
    rt.update(|s, tx| s.open_reply_link("docs/proposal.md:4", tx));
    settle(&mut rt);
    assert!(!shows_rendered(&rt.state.files.sessions[THIRD], PROPOSAL));

    rt.update(|s, tx| s.open_reply_link("docs/nope.md", tx));
    settle(&mut rt);
    let toast = rt.state.toast.current().cloned().expect("a toast");
    assert_eq!((toast.title.as_str(), toast.description.as_deref()), ("File not found", Some("docs/nope.md")));
    assert!(!rt.state.files.sessions[THIRD].docs.contains_key("docs/nope.md"));
    assert_eq!(active_file(&rt).and_then(|(_, p)| p).as_deref(), Some(PROPOSAL));
}

#[test]
fn a_bare_word_link_opens_the_file_when_it_exists_and_says_so_when_it_does_not() {
    let mut rt = runtime();
    rt.update(|s, tx| s.open_reply_link("Makefile", tx));
    settle(&mut rt);
    assert!(rt.state.toast.current().is_none());
    assert_eq!(active_file(&rt).and_then(|(_, p)| p).as_deref(), Some("Makefile"));

    rt.update(|s, tx| s.open_reply_link("link", tx));
    settle(&mut rt);
    let toast = rt.state.toast.current().cloned().expect("a toast");
    assert_eq!((toast.title.as_str(), toast.description.as_deref()), ("This link doesn't point anywhere", Some("link")));
    assert!(!rt.state.open_in.busy());
    assert_eq!(active_file(&rt).and_then(|(_, p)| p).as_deref(), Some("Makefile"));
}

#[test]
fn files_outside_the_folder_open_in_their_app_and_web_links_open_no_tab() {
    let mut rt = runtime();
    rt.update(|s, tx| s.open_reply_link("/Users/demo/code/harbor/README.md", tx));
    assert!(rt.state.open_in.busy(), "handed to the default app");
    assert!(rt.state.toast.current().is_none());
    rt.update(|s, tx| s.open_reply_link("https://example.com/docs/proposal.md", tx));
    settle(&mut rt);
    assert!(active_file(&rt).is_none());
}

#[test]
fn the_toggle_switches_between_rendered_and_source_per_file() {
    let mut rt = runtime();
    open(&mut rt, PROPOSAL);
    let md_key = super::markdown::scroll_key(THIRD, PROPOSAL);
    assert!(rt.find(&md_key).is_some());

    click(&mut rt, "files-render-btn");
    assert!(!shows_rendered(&rt.state.files.sessions[THIRD], PROPOSAL));
    assert!(rt.find(&md_key).is_none());

    // Another file keeps its own view; coming back keeps this one's.
    open(&mut rt, PRINCIPLES);
    assert!(rt.find(&super::markdown::scroll_key(THIRD, PRINCIPLES)).is_some());
    open(&mut rt, PROPOSAL);
    assert!(rt.find(&md_key).is_none());

    click(&mut rt, "files-render-btn");
    assert!(rt.find(&md_key).is_some());
    // Code files have no toggle.
    open(&mut rt, "src/net/stream.ts");
    assert!(rt.find("files-render-btn").is_none());
}

#[test]
fn find_searches_the_rendered_text() {
    let mut rt = runtime();
    open(&mut rt, PROPOSAL);
    rt.update(|s, _| {
        s.side.focused = true;
        let d = s.files.sessions.get_mut(THIRD).unwrap().docs.get_mut(PROPOSAL).unwrap();
        d.find_open = true;
        d.find_input = TextEdit::new("store");
        d.find_input.focused = true;
    });
    settle(&mut rt);
    let v = &rt.state.files.sessions[THIRD].md[PROPOSAL];
    // Rendered text: `Store` in the table counts; Markdown syntax doesn't.
    assert!(v.matches.len() >= 6, "{}", v.matches.len());
    rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
    settle(&mut rt);
    assert_eq!(rt.state.files.sessions[THIRD].md[PROPOSAL].active, 1);
    assert!(rt.find("find-active").is_some());
}

#[test]
fn a_heading_link_scrolls_the_rendered_file() {
    let mut rt = runtime();
    open(&mut rt, PROPOSAL);
    let (_, before) = rt.find("mdh-the-handler").expect("heading keyed");
    rt.update(|s, tx| s.open_link(THIRD, Some(PROPOSAL), "#the-handler", tx));
    settle(&mut rt);
    let (_, after) = rt.find("mdh-the-handler").unwrap();
    assert!(after < before - 100.0, "heading scrolled up: {before} → {after}");
}
