use rune::{Env, Runtime};

use super::external_url;
use crate::panels::panel_key;
use crate::{App, spec};

const DT: f64 = 1.0 / 60.0;

fn with_tab(url: &str) -> (Runtime<App>, u64) {
    let s = spec(App::fixture_session);
    let mut rt = Runtime::new((s.init)(), s.view, s.size, Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    let id = rt.state.open_new_browser_tab(url);
    if !rt.state.side_open() {
        rt.update(|s, tx| s.toggle_side_region(tx));
    }
    for _ in 0..40 {
        rt.frame(DT);
    }
    (rt, id)
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
}

#[test]
fn only_web_pages_open_outside() {
    assert_eq!(external_url("https://example.com/a?b#c"), Some("https://example.com/a?b#c"));
    assert_eq!(external_url("http://localhost:5173/"), Some("http://localhost:5173/"));
    assert_eq!(external_url("HTTPS://example.com/"), Some("HTTPS://example.com/"));
    for url in ["", "about:blank", "chrome://settings", "data:text/html,hi", "file:///tmp/a.html", "javascript:alert(1)"] {
        assert_eq!(external_url(url), None, "{url}");
    }
}

#[test]
fn the_address_bar_button_opens_the_page_in_the_default_browser() {
    let (mut rt, _) = with_tab("http://localhost:4792/");
    tap(&mut rt, "browser-open-out");
    assert_eq!(rt.take_url_requests(), ["http://localhost:4792/"]);
}

#[test]
fn a_blank_tab_has_nothing_to_open() {
    let (mut rt, _) = with_tab("about:blank");
    tap(&mut rt, "browser-open-out");
    assert!(rt.take_url_requests().is_empty());
}

#[test]
fn the_tab_menu_opens_the_page_and_copies_its_address() {
    let (mut rt, id) = with_tab("https://example.com/docs");
    let key = panel_key("browser", &id.to_string());
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);

    rt.update(|s, _| s.side.tab_menu = Some((key.clone(), 100.0, 100.0)));
    rt.frame(DT);
    tap(&mut rt, "side-tab-menu-open-out");
    assert_eq!(rt.take_url_requests(), ["https://example.com/docs"]);
    assert!(rt.state.side.tab_menu.is_none(), "picking closes the menu");

    rt.update(|s, _| s.side.tab_menu = Some((key.clone(), 100.0, 100.0)));
    rt.frame(DT);
    tap(&mut rt, "side-tab-menu-copy-url");
    assert_eq!(clipboard.text().map(|t| t.to_string()).as_deref(), Some("https://example.com/docs"));
    assert!(rt.take_url_requests().is_empty());
}
