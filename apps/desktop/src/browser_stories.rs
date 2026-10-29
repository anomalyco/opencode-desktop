//! Storybook stages (`⌘⇧D`) for the Browser split pane.

use browser_pane::EngineStatus;
use rune::{El, Play};

use crate::App;
use crate::palette::Pal;
use crate::stories::{Trigger, replay};

pub const VARIANTS: &[(&str, usize)] = &[
    ("Active tab", 0),
    ("Multiple tabs", 1),
    ("Element picker", 2),
    ("Installing engine", 3),
];

pub const KEYS: &[Trigger] = &[
    replay(setup_variant),
    Trigger {
        key: 't',
        label: "New tab",
        run: |s, _| {
            s.open_new_browser_tab("https://github.com/anomalyco/rune");
        },
    },
    Trigger {
        key: 'i',
        label: "Toggle element picker",
        run: |s, _| {
            if let Some(id) = s.browser.host.focused_tab_id() {
                s.browser.host.toggle_inspect(id);
            }
        },
    },
    Trigger {
        key: 'w',
        label: "Toggle wide split",
        run: |s, _| {
            s.browser.width = if s.browser.width > 600.0 { 480.0 } else { 680.0 };
        },
    },
];

pub fn setup_variant(s: &mut App, _play: &mut Play<'_, App>) {
    s.finish_stream();
    s.browser = Default::default();
    s.browser.open = true;
    match s.variant {
        0 => {
            s.browser.host.insert_mock_tab(
                "http://localhost:3000/dashboard",
                "Local Preview",
                EngineStatus::Ready,
            );
        }
        1 => {
            s.browser.host.insert_mock_tab(
                "http://localhost:3000/",
                "Local Preview",
                EngineStatus::Ready,
            );
            s.browser.host.insert_mock_tab(
                "https://github.com/anomalyco/rune",
                "anomalyco/rune",
                EngineStatus::Ready,
            );
        }
        2 => {
            let id = s.browser.host.insert_mock_tab(
                "http://localhost:5173/settings",
                "Settings App",
                EngineStatus::Ready,
            );
            s.browser.host.toggle_inspect(id);
        }
        _ => {
            s.browser.host.insert_mock_tab(
                "http://localhost:3000/",
                "New tab",
                EngineStatus::Downloading {
                    downloaded_bytes: 54 * 1024 * 1024,
                    total_bytes: Some(124 * 1024 * 1024),
                },
            );
        }
    }
    s.sync_browser_address_now();
}

pub fn view(s: &App, p: &Pal) -> El {
    crate::browser::view::panel_card(s, p, s.browser.width.min(680.0), 520.0)
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story {
        path: "Browser/Pane",
        scene: |s| crate::stories::stage(s, view),
        knobs: &[],
        keys: KEYS,
        variants: VARIANTS,
    },
]);
