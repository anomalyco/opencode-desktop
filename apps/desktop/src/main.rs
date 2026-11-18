//! OpenCode Desktop in Rune: OpenCode's home and session pages in Rune, live against the local OpenCode server or
//! from a synthetic fixture. Live, it sends prompts, answers permission requests and questions,
//! stops runs and starts sessions (`--read-only` turns all of that off).
// apps/web builds this crate for the browser; built by it for any other target, it's empty.
#![cfg(not(opencode_web_host))]

/// Counts each thread's allocations for the frame diagnostics and the benchmarks, over mimalloc:
/// the system heap costs a Luau plugin's rebuild (its tables, strings and elements) about a fifth
/// more on Windows.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static ALLOC: procwatch::alloc::Counting<launch::last_exit::OomGuard<mimalloc::MiMalloc>> = procwatch::alloc::Counting::new(launch::last_exit::OomGuard(mimalloc::MiMalloc));

/// mimalloc commits each medium and large page whole when a thread heap takes it; on demand it
/// commits a page only as blocks are carved from it. Every thread that allocates holds such pages
/// for its size classes, mostly untouched: on Home this is 60-75 MB of commit charge that is never
/// written (private bytes ~205 MB to ~145 MB, the private working set unchanged).
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
fn commit_pages_on_demand() {
    unsafe extern "C" {
        fn mi_version() -> std::ffi::c_int;
        fn mi_option_set(option: std::ffi::c_int, value: std::ffi::c_long);
    }
    // `mi_option_page_commit_on_demand` is 40 in mimalloc 3.3's `mi_option_t` (mimalloc.h); a
    // later mimalloc may number it otherwise, so it's only set on the version it was read from.
    // SAFETY: plain calls into the linked mimalloc; an option set after start applies to the
    // pages taken from then on.
    unsafe {
        if mi_version() / 100 == 303 {
            mi_option_set(40, 1);
        }
    }
}

/// `App::rest`: mimalloc collects this thread's heap and purges every free page now; on Windows
/// the process heap compacts too.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
pub(crate) fn purge_free_pages() {
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    // SAFETY: a plain call into the linked mimalloc, on the UI thread.
    unsafe { mi_collect(true) }
    // The process heap (UI Automation, COM, the D3D runtime and driver) keeps freed pages
    // committed; compacting it gives its large free blocks back.
    #[cfg(windows)]
    // SAFETY: the process heap lives as long as the process; HeapCompact takes its lock.
    unsafe {
        windows_sys::Win32::System::Memory::HeapCompact(windows_sys::Win32::System::Memory::GetProcessHeap(), 0);
    }
}

// A browser's requests speak ureq's API too (ureq-web), so the request code is the same there.
#[cfg(target_arch = "wasm32")]
extern crate ureq_web as ureq;

pub use session_view::{
    agent_start, disclosure, fold, jump, line_reveal, odometer, palette, question_row,
    subagent_card as agent_rows_view, themes, turns,
};

mod compaction {
    pub use session_view::compaction::*;
}
mod copied {
    pub use session_view::copied::*;
}
mod turn_errors {
    pub use session_view::turn_errors::*;
}
mod working {
    pub use session_view::working::*;
}

#[allow(unused_imports)]
pub(crate) use opencode_platform::{askpass, telemetry, wall, wall_ms, web_fetch};

mod app;
mod dialogs;
mod kernel;
mod plugin_host;
mod session;
mod shell;
mod stories;
mod surfaces;
mod tick;
mod workspace;

pub use app::{App, Source, TabStatus};
pub(crate) use app::{FIXTURE, arg, attended, auto_focus_composer, flag, harness_writes, headless};
pub use shell::nav::route::Route;
pub(crate) use stories::scenes::open_scene;
pub(crate) use tick::{poll, setup, visibility};

// Domain submodule re-exports at crate root so internal `crate::<module>::*` paths resolve cleanly.
pub use shell::panels;
pub use surfaces::code_view;

#[allow(unused_imports)]
pub(crate) use shell::{
    appearance,
    appearance::{background, knobs, language, text_rendering},
    chrome::{compositor, forms, glyph_masks, icons, scrollbar, status_toast, wheel, widgets},
    commands,
    commands::type_to_compose,
    diag::{diagnostics, motion_log},
    input::{escape, keymap, keys, shortcuts},
    menus::{add_menu, app_menu, menubar, popover, popup},
    narrow,
    nav::{history, layout_state, pinch, route},
    tabs,
    tabs::{actions as tab_actions, badges as tab_badges, recently_closed},
    view,
};

#[allow(unused_imports)]
pub(crate) use session::{
    composer::{
        add_selection, attachments, dictation, dropzone, line_comments, mentions, pickers,
        prompt_history, quote, quote::cards as quote_cards, slash, suggestions, undo,
    },
    events,
    events::{incoming, stream::pace as stream_pace},
    inbox::{
        model as inbox_model, order as inbox_order, queue, send, send_failed, stop, submit,
    },
    menu as session_menu,
    menu::view as session_menu_view,
    meta::{
        catalog as session_catalog, context_window, cost as session_cost, open as session_open,
        state as session_state, status as session_status, title_change, turn_diffs,
    },
    pages as session_pages,
    timeline::{
        agent_rows, auto_accept, diagram, find, fork, genui, message_nav, permission, question,
        rewind, shell_fold, short_group, subagents, todos, tool_open,
    },
};

#[allow(unused_imports)]
pub(crate) use workspace::{
    lifecycle::{
        idle, jobs, launch, new_session, new_worktree, page_sign_in, reconnect, restore, rune_mcp,
        served, storage, tips,
    },
    projects,
    search::{fulltext, home_search, project_switcher, recent_page, switcher, transcript_index},
    servers,
    system::{
        crash_notice, deep_links, dirs, errors, export_logs, jump_list, notifications, open_in,
        sounds, speech,
    },
    windows,
};
#[cfg(windows)]
pub(crate) use workspace::system::install;

#[allow(unused_imports)]
pub(crate) use surfaces::{browser, egg, files, glitch, review, terminal, updater};
#[cfg(feature = "office")]
pub(crate) use surfaces::office;

#[allow(unused_imports)]
pub(crate) use dialogs::{connect, mcp, model_picker, model_prefs, settings, usage};

#[allow(unused_imports)]
pub(crate) use stories::{demo, demo_script, fixture_ids, open as open_story, scenes};

// Crate-root type imports kept for internal modules using `use crate::*;`.
#[allow(unused_imports)]
pub(crate) use std::collections::{HashMap, HashSet, VecDeque};
#[allow(unused_imports)]
pub(crate) use std::sync::mpsc::{Receiver, Sender};
#[allow(unused_imports)]
pub(crate) use rune::{AppSpec, Key, KeyEvent, TextEdit, Tx, Waker};
#[allow(unused_imports)]
pub(crate) use serde_json::Value;
#[allow(unused_imports)]
pub(crate) use opencode_kit::client::{DEFAULT_SERVER, Incoming, Prompt, Request, Server, Write};
#[allow(unused_imports)]
pub(crate) use opencode_kit::servers::ServerKey;
#[allow(unused_imports)]
pub(crate) use opencode_kit::model::{Delivery, Inbox, Message, Pending, Project, SessionInfo, Transcript};
#[allow(unused_imports)]
pub(crate) use tabs::Tab;
#[allow(unused_imports)]
pub(crate) use pickers::PickerHost;

/// Configures extracted feature crates once per process (native and wasm32).
pub(crate) fn init_features() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        keymap::init();
        updater::init();
        settings::init();
        #[cfg(feature = "office")]
        office::init();
    });
}

/// The app's fonts. Desktop sets every IBM Plex Mono weight below 600 in its Text cut and the rest
/// in its Bold cut (`index.css`), which a Markdown heading in a diff takes, and italic Plex in its
/// Text Italic cut (inline code in emphasis). JetBrains Mono is desktop's terminal face
/// (`terminalDefault`), Nerd Font glyphs included for prompts.
#[cfg(not(target_arch = "wasm32"))]
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/InterVariable.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Text.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-TextItalic.ttf"),
    include_bytes!("../assets/fonts/CommitMono-400-Regular.otf"),
    include_bytes!("../assets/fonts/CommitMono-700-Regular.otf"),
    include_bytes!("../assets/fonts/CommitMono-400-Italic.otf"),
    include_bytes!("../assets/fonts/CommitMono-700-Italic.otf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFontMono-Bold.ttf"),
];
/// In a browser, only the fonts the default appearance draws in: the page fetches Plex's italic and
/// Commit Mono once its first frame is up (`LATE_FONTS`), and JetBrains Mono, the terminal's (4.8 MB), when a
/// terminal first attaches (`TERMINAL_FONTS`).
#[cfg(target_arch = "wasm32")]
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/InterVariable.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Text.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
];
/// The fonts a browser fetches after its first frame, which the web build serves from `fonts/`
/// beside the page (`scripts/web.ps1`).
#[cfg(target_arch = "wasm32")]
pub(crate) const LATE_FONTS: &[&str] = &[
    "fonts/IBMPlexMono-TextItalic.ttf",
    "fonts/CommitMono-400-Regular.otf",
    "fonts/CommitMono-700-Regular.otf",
    "fonts/CommitMono-400-Italic.otf",
    "fonts/CommitMono-700-Italic.otf",
];
/// The terminal's face, which a page fetches when its first terminal attaches (`terminal::fetch_terminal_font`).
#[cfg(target_arch = "wasm32")]
pub(crate) const TERMINAL_FONTS: &[&str] = &["fonts/JetBrainsMonoNerdFontMono-Regular.ttf", "fonts/JetBrainsMonoNerdFontMono-Bold.ttf"];

/// The app's spec: `init` makes its state (`App::new`, or `App::empty` in tests).
pub(crate) fn spec(init: fn() -> App) -> AppSpec<App> {
    init_features();
    rune::text::set_face_weights(appearance::FACE_WEIGHTS);
    // Desktop's base.css sets `text-wrap-style: pretty` on every element.
    rune::text::set_pretty_wrap(true);
    AppSpec {
        scenes: stories::scenes(),
        fonts: FONTS,
        families: (appearance::UI_FONTS[0].family, appearance::CODE_FONTS[0].family),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(keys::on_key),
        ..AppSpec::new(launch::window_title(), (1280.0, 820.0), init, view::app)
    }
}

/// The app as it runs: its history, its storybook (⌘⇧D), its window chrome, and Rune's native
/// Metal backend (`RUNE_GPU_BACKEND=wgpu` draws with wgpu again, no rebuild).
pub(crate) fn rune_app(spec: AppSpec<App>) -> rune::App<App> {
    let app = rune::App::new(spec);
    // Office pages draw their Vello scenes into effect slots, and the new-session Easter egg its
    // pins and lodger; a window without one draws as Rune's default frame does.
    let app = app.compositor(compositor::AppCompositor::default());
    menubar::install(
        app
            .gpu_backend(rune::GpuBackend::Metal)
            .playground(stories::playground())
            .on_ime(|s, ime, _| s.panels_ime(ime))
            .on_open_url(deep_links::open)
            // The page's server's copy of the session data moves into the window whose turn it is.
            .turn_scope(servers::runtime::enter, servers::runtime::leave)
            .on_modifiers(view::home_modifiers)
            .on_visibility(visibility)
            // Tab reaches every control on a Mac whatever its Keyboard navigation setting says,
            // as official's page does in Chromium: with the setting off (macOS's default), shipped
            // 2.2.7 Tabs through Home, Search sessions, New session, a reply's buttons and the
            // prompt, where AppKit's rule left only the prompt and the tab strip.
            .tab_visits_controls(true)
            .titlebar(rune::Titlebar::new(view::TITLEBAR_H))
            .chrome(rune::Chrome {
                full_size_content: true,
                traffic_lights: Some((20.0, 25.0)),
                drag_regions: true,
            }),
    )
}

/// The browser build's entry (apps/web).
#[cfg(target_arch = "wasm32")]
#[path = "../../web/src/web.rs"]
mod web;

#[cfg(not(any(target_arch = "wasm32", feature = "hot")))]
thread_local! {
    /// The launch's frame diagnostics, for `quit_last` to drain.
    static QUIT_DIAGNOSTICS: std::cell::RefCell<Option<diagnostics::Diagnostics>> = const { std::cell::RefCell::new(None) };
}

/// The windows' last step on the way out (`rune::fast_exit`), on the UI thread once every window's
/// quit hook ran: the database's last write, the diagnostics' queue, an install-on-quit's helper
/// (it must exist before this process ends), the launch trace, then a training build's profile.
/// Rune's fast exit raises every thread to normal priority before the process ends itself, so an
/// idle one can't hold it.
#[cfg(not(any(target_arch = "wasm32", feature = "hot")))]
fn quit_last() {
    // Landed and closed: the file's lock goes before the update helper takes over.
    storage::close();
    rune::marks::mark("storage.flushed");
    // Nothing of the app's is left to lose: whatever the rest waits on, the process ends within
    // 2 s, saying what it waited for. A training build (`scripts/pgo.ps1`) writes ~50 MB of counts
    // last; on a loaded machine that outlasted 2 s and left truncated, unmergeable files.
    rune::exit_deadline(std::time::Duration::from_secs(if cfg!(pgo_generate) { 120 } else { 2 }));
    // A launch handed on while the app quit claims it now, unless an update replaces it.
    if !updater::installs_on_quit() {
        rune::release_instance();
    }
    QUIT_DIAGNOSTICS.with(|d| if let Some(d) = d.borrow_mut().take() { d.shutdown() });
    rune::marks::mark("diagnostics.stopped");
    updater::finish_quit();
    rune::marks::mark("quit.update_handed_off");
    launch::trace::finish(windows::opened());
    launch::write_profile();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    commit_pages_on_demand();
    #[cfg(feature = "alloc-sites")]
    plugin_host::memory::register_reports();
    // A debug build COM starts for a proof's scratch toast activator (`LocalServer32`, whose
    // command line is fixed in the registry and whose environment is the user's) takes its
    // scratch root from the command line: it never runs without one. Release builds ignore it.
    #[cfg(debug_assertions)]
    if let Some(root) = std::env::args().find_map(|a| a.strip_prefix("--dirs-root=").map(str::to_owned)) {
        // SAFETY: the first thing `main` does, before any other thread exists.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var("RUNE_DIRS_ROOT", root) };
    }
    launch::trace::begin();
    telemetry::launched();
    launch::attach_console();
    askpass::run_if_asked();
    init_features();
    #[cfg(windows)]
    updater::apply::run_if_asked();
    #[cfg(windows)]
    install::run_if_asked();
    launch::print_version_if_asked();
    if plugin_host::types::from_args() {
        return;
    }
    if genui::skill::maybe_cli() {
        return;
    }
    // The app's state, read on its own thread beside the instance claim (`storage::start`). A
    // launch while the app runs hands its links (`deep_links`) to it and ends here.
    storage::start();
    if !headless() && !rune::single_instance(&launch::instance_id()) {
        return launch::trace::finish(0);
    }
    storage::claimed();
    launch::last_exit::init_hooks();
    notifications::take_activation(rune::extra_args());
    glyph_masks::start();
    // The first view's theme tokens (OpenCode's kit and the default theme's palette, both
    // appearances from one parse), built while the window is made rather than in its first frame.
    let _ = std::thread::Builder::new().name("theme-warm".into()).spawn(|| {
        for dark in [true, false] {
            session_view::palette::kit(session_view::palette::pal(dark));
        }
    });
    opencode_kit::client::set_download_dir(dirs::downloads());
    // Where the browser engine lives follows this app's data root (no I/O until a page opens).
    #[cfg(any(target_os = "macos", windows))]
    if let Some(engine) = dirs::engine() {
        browser_pane::installer::set_home(engine);
    }
    telemetry::startup("launch.platform", rune_mac::launch);
    telemetry::startup("launch.notifications", notifications::identify);
    telemetry::startup("launch.gpu", rune::prewarm_gpu);
    // Windows: no GPU before the first frame. Rune paints it on the CPU and makes the window's GPU
    // on a below-normal thread once it's on screen, then hands over. Warming the GPU here put its
    // driver loads beside the launch's: the first frame came 8 ms and the first input 15 ms later
    // (p50, visible desktop), for a hand-over ~50 ms sooner that nobody sees.
    telemetry::startup("launch.icon", launch::apply_app_icon);
    #[cfg(feature = "hot")]
    rune_app(AppSpec { families: appearance::launch_families(), ..spec(App::hot) }).run();
    #[cfg(not(feature = "hot"))]
    {
        let diagnostics = diagnostics::Diagnostics::default();
        if !headless() {
            launch::defer();
            rune::marks::mark("windows.launch");
            // Quitting ends the process right after `quit_last`: no window, GPU device or allocation
            // is torn down (on Windows that was the graphics drivers' detach, hundreds of ms).
            QUIT_DIAGNOSTICS.with(|d| *d.borrow_mut() = Some(diagnostics.clone()));
            rune::fast_exit(quit_last);
            // Every window saved at the last quit, each with its own tabs (`windows`).
            windows::launch(diagnostics);
            quit_last();
            return;
        }
        // The harness's `--realtime` probes launch as a window does: the server's first reads
        // wait for `on_ready`, which only a realtime run calls (after its first frame).
        if harness_writes() && std::env::args().any(|a| a == "--realtime") {
            launch::defer();
        }
        let app = rune_app(AppSpec { families: appearance::launch_families(), ..spec(App::new) });
        let handle = app.handle();
        let mut motion_log = motion_log::Log::default();
        let (encoded, drawn) = (diagnostics.clone(), diagnostics.clone());
        let app = app
            .on_ready(move |_| {
                storage::presented();
                let wake = launch::inbox_waker(handle.clone());
                handle.update_if(move |s, tx| { s.launch.inbox_wake = Some(wake); launch::ready(s, tx) });
            })
            .on_frame_encoded(move |f| {
                encoded.encoded(f);
                telemetry::encoded(f);
                files::trace::frame(f);
            })
            .on_frame_drawn(move |rt| {
                drawn.drawn(rt);
                if let Some(frame) = telemetry::last_frame() { telemetry::drawn(rt.state.painted_session(), frame); }
                motion_log.frame(rt);
            })
            .on_close(|s, _| {
                let scheduler = s.kernel.scheduler.clone();
                scheduler.run_close(s);
                s.save_on_close();
            })
            .on_rest(App::rest);
        app.run();
        storage::close();
        diagnostics.shutdown();
        // A headless run's marks too (`--launch-trace`): the harness's realtime probes, such as the
        // browser engine's timing.
        launch::trace::finish(1);
    }
}
