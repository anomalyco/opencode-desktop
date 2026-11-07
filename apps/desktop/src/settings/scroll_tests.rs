//! Every Settings page scrolls under the wheel in a window too short for it, in each tab layout.

use rune::Runtime;

use super::pages::Page;
use crate::App;
use crate::layout_state::TabLayout;

const DT: f64 = 1.0 / 60.0;
const SIZE: (f32, f32) = (1100.0, 420.0);

fn settings(layout: TabLayout, page: &Page) -> Runtime<App> {
    let mut a = App::fixture_session();
    a.settings.tab_layout = layout;
    let mut rt = a.into_runtime_sized(SIZE);
    rt.frame(0.0);
    rt.state.open_settings();
    rt.state.show_settings_page(page.clone());
    rt.invalidate();
    for _ in 0..30 { rt.frame(DT); }
    rt
}

fn rect(rt: &Runtime<App>, key: &str) -> rune::ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}"))
}

fn wheel(rt: &mut Runtime<App>, dy: f32) {
    let v = rect(rt, "settings-content").visible;
    for _ in 0..10 {
        rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, dy);
        rt.frame(DT);
    }
    for _ in 0..60 { rt.frame(DT); }
}

#[test]
fn every_settings_page_scrolls_in_every_tab_layout() {
    let pages: Vec<Page> = super::shell::groups(&App::fixture_session()).into_iter().flatten().collect();
    let mut scrolled = Vec::new();
    for layout in [TabLayout::Horizontal, TabLayout::Vertical, TabLayout::Inbox] {
        for page in &pages {
            let mut rt = settings(layout, page);
            let area = rect(&rt, "settings").rect;
            assert!(area[1] + area[3] <= SIZE.1 + 0.5, "{layout:?} {page:?}: the scroll area fits the window: {area:?}");
            let content = rect(&rt, "settings-content").rect;
            if content[1] + content[3] <= area[1] + area[3] + 1.0 {
                continue;
            }
            let before = rect(&rt, "settings-content").scrolled[1];
            wheel(&mut rt, -120.0);
            let down = rect(&rt, "settings-content").scrolled[1];
            assert!((down - before).abs() > 4.0, "{layout:?} {page:?}: the wheel scrolls it ({before} → {down})");
            wheel(&mut rt, 120.0);
            assert!((rect(&rt, "settings-content").scrolled[1] - before).abs() < 1.0, "{layout:?} {page:?}: and back");
            scrolled.push((layout, page.clone()));
        }
    }
    for layout in [TabLayout::Horizontal, TabLayout::Vertical, TabLayout::Inbox] {
        assert!(scrolled.contains(&(layout, Page::General)), "{layout:?}: General overflows a short window, so it was checked");
    }
}
