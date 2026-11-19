//! `--scene browser`: the fixture's session with two pages in its side panel and the keyboard in
//! the composer, for driving the pane's keys headless (`--script`).

use browser_pane::EngineStatus;

use crate::App;

fn scene(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    for (url, title) in [("http://localhost:3000/", "Local Preview"), ("https://github.com/anomalyco/rune", "anomalyco/rune")] {
        let id = s.workspace_panes.browser.host.insert_mock_tab(url, title, EngineStatus::Ready);
        s.register_existing_browser_tab(id);
    }
    s.workspace_panes.side.focus.blur();
    s.composer.focused = true;
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("browser", scene)]);
