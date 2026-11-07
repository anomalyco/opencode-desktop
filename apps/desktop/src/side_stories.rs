//! Storybook stages (`⌘⇧D`) for the Side Region (`Side panel/Side region`) and `Browser/Pane`.

use browser_pane::EngineStatus;
use rune::{El, Play};

use crate::App;
use crate::palette::Pal;
use crate::panels::tabs::{ClosedSideTab, panel_key, push_closed_tab};
use crate::panels::SessionPane;
use crate::stories::{Story, Trigger, replay, stage};

pub const REGION_VARIANTS: &[(&str, usize)] = &[
    ("Multiple tabs", 0),
    ("New tab and recently closed", 1),
    ("Split panes", 2),
];

pub const BROWSER_VARIANTS: &[(&str, usize)] = &[
    ("Active tab", 0),
    ("Multiple tabs", 1),
    ("Element picker", 2),
];

pub const INSTALL_VARIANTS: &[(&str, usize)] = &[
    ("Download prompt", 0),
    ("Downloading", 1),
    ("Download failed", 2),
    ("No GitHub login", 3),
    ("Verifying", 4),
    ("Paused on battery", 5),
];

pub const INSTALL_KEYS: &[Trigger] = &[
    replay(setup_install),
    Trigger {
        key: 'p',
        label: "Step download +19 MB",
        run: |s, play| {
            play.animated();
            let total = 128_352_035u64;
            let next = match s.browser.host.status() {
                EngineStatus::Downloading { downloaded_bytes, .. } => {
                    (downloaded_bytes + 19_000_000).min(total)
                }
                _ => 19_000_000,
            };
            s.browser.host.set_mock_status(EngineStatus::Downloading {
                downloaded_bytes: next,
                total_bytes: Some(total),
            });
        },
    },
];

pub const REGION_KEYS: &[Trigger] = &[
    replay(setup_region),
    Trigger {
        key: 'b',
        label: "Toggle side panel",
        run: |s, play| {
            play.animated();
            let mut tx = rune::Tx::default();
            s.toggle_side_region(&mut tx);
        },
    },
    Trigger {
        key: 't',
        label: "Next tab",
        run: |s, _| {
            let mut tx = rune::Tx::default();
            let _ = s.select_adjacent_side_tab(1, &mut tx);
        },
    },
    Trigger {
        key: 'n',
        label: "Open New tab",
        run: |s, _| {
            s.open_side_new_tab(SessionPane::Side);
        },
    },
    Trigger {
        key: 'w',
        label: "Toggle wide split",
        run: |s, play| {
            play.animated();
            // The side region takes what the chat panel leaves: widen the chat to narrow it.
            if let Some(sid) = s.active_session_id().map(str::to_owned) {
                let st = s.session_side_mut(&sid);
                st.session_width = if st.session_width.is_some() { None } else { Some(856.0) };
            }
        },
    },
];

pub const BROWSER_KEYS: &[Trigger] = &[
    replay(setup_browser),
    Trigger {
        key: 't',
        label: "New browser tab",
        run: |s, _| {
            s.open_new_browser_tab("https://github.com/anomalyco/rune");
        },
    },
    Trigger {
        key: 'i',
        label: "Toggle element picker",
        run: |s, _| {
            if let Some(id) = s.active_browser_tab_id() {
                s.browser.host.toggle_inspect(id);
            }
        },
    },
];

pub fn init_region(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.side = Default::default();
    s.browser = Default::default();
    let Some(sid) = s.active_session_id().map(str::to_owned) else { return };

    match s.variant {
        0 => {
            s.open_new_browser_tab("http://localhost:3000/dashboard");
            s.open_new_browser_tab("https://github.com/anomalyco/rune");
        }
        1 => {
            s.open_new_browser_tab("http://localhost:3000/");
            let now = s.now_ms;
            push_closed_tab(
                &mut s.session_side_mut(&sid).closed,
                ClosedSideTab {
                    key: "browser:99".into(),
                    title: "localhost:5173/settings".into(),
                    icon_name: Some("globe".into()),
                    file: None,
                    detail: Some("localhost:5173".into()),
                    data: Some("http://localhost:5173/settings".into()),
                    closed_ms: now,
                },
            );
            s.open_side_new_tab(SessionPane::Side);
        }
        _ => {
            let t1 = s.open_new_browser_tab("http://localhost:3000/");
            let t2 = s.open_new_browser_tab("https://github.com/anomalyco/rune");
            let mut tx = rune::Tx::default();
            s.move_side_tab(&panel_key("browser", &t2.to_string()), SessionPane::Split, None, &mut tx);
            s.select_side_tab(&panel_key("browser", &t1.to_string()), &mut tx);
        }
    }
    s.sync_browser_address_now();
}

pub fn setup_region(s: &mut App, _play: &mut Play<'_, App>) {
    init_region(s);
}

pub fn init_browser(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.side = Default::default();
    s.browser = Default::default();
    match s.variant {
        0 => {
            let id = s.browser.host.insert_mock_tab(
                "http://localhost:3000/dashboard",
                "Local Preview",
                EngineStatus::Ready,
            );
            s.register_existing_browser_tab(id);
        }
        1 => {
            let id1 = s.browser.host.insert_mock_tab(
                "http://localhost:3000/",
                "Local Preview",
                EngineStatus::Ready,
            );
            s.register_existing_browser_tab(id1);
            let id2 = s.browser.host.insert_mock_tab(
                "https://github.com/anomalyco/rune",
                "anomalyco/rune",
                EngineStatus::Ready,
            );
            s.register_existing_browser_tab(id2);
        }
        _ => {
            let id = s.browser.host.insert_mock_tab(
                "http://localhost:5173/settings",
                "Settings App",
                EngineStatus::Ready,
            );
            s.register_existing_browser_tab(id);
            s.browser.host.toggle_inspect(id);
        }
    }
    s.sync_browser_address_now();
}

pub fn setup_browser(s: &mut App, _play: &mut Play<'_, App>) {
    init_browser(s);
}

pub fn init_install(s: &mut App) {
    crate::open_scene(s);
    s.finish_stream();
    s.side = Default::default();
    s.browser = Default::default();
    let status = match s.variant {
        0 => EngineStatus::NotInstalled,
        1 => EngineStatus::Downloading {
            downloaded_bytes: 58_400_000,
            total_bytes: Some(128_352_035),
        },
        2 => EngineStatus::Failed(browser_pane::ERR_DOWNLOAD.into()),
        4 => EngineStatus::Verifying,
        5 => EngineStatus::Paused(browser_pane::PauseReason::OnBattery),
        _ => EngineStatus::Failed(browser_pane::ERR_SIGN_IN.into()),
    };
    let id = s
        .browser
        .host
        .insert_mock_tab("http://localhost:3000/", "Local Preview", status);
    s.register_existing_browser_tab(id);
    s.sync_browser_address_now();
}

pub fn setup_install(s: &mut App, _play: &mut Play<'_, App>) {
    init_install(s);
}

pub fn view_card(s: &App, p: &Pal) -> El {
    crate::panels::view::story_card(s, p, s.side_width(s.side.laid_w.get()).min(640.0), 540.0)
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Side panel/Side region",
        scene: |s| {
            init_region(s);
            stage(s, view_card);
        },
        knobs: &[],
        keys: REGION_KEYS,
        variants: REGION_VARIANTS,
    },
    Story {
        path: "Browser/Pane",
        scene: |s| {
            init_browser(s);
            stage(s, view_card);
        },
        knobs: &[],
        keys: BROWSER_KEYS,
        variants: BROWSER_VARIANTS,
    },
    Story {
        path: "Browser/Engine install",
        scene: |s| {
            init_install(s);
            stage(s, view_card);
        },
        knobs: &[],
        keys: INSTALL_KEYS,
        variants: INSTALL_VARIANTS,
    },
]);
