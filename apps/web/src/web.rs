//! The browser entry: the desktop app's window, as the page. The page's query names the server
//! (`?server=http://127.0.0.1:4096`) and the page-safe options [`page_args`] accepts (`?session=…`,
//! `?debug`, `?zoom=1.25`, `?theme=dark`); unknown or malformed query parameters are ignored so a
//! shared link never traps the module. The page (index.html) takes a password out of its address
//! before it loads this, and hands it to [`start`].

use opencode_desktop::{App, web_api};
use wasm_bindgen::{JsCast, closure::Closure, prelude::wasm_bindgen};

use crate::browser_history;

#[wasm_bindgen(inline_js = "export function initialPointer() {\n  const hoverNone = matchMedia('(hover: none)').matches;\n  const coarse = matchMedia('(pointer: coarse)').matches;\n  return (hoverNone ? 1 : 0) | (hoverNone && coarse ? 2 : 0);\n}\nexport function initialCpuCanvasOnly() {\n  return !(globalThis.navigator && 'gpu' in globalThis.navigator) || globalThis.RUNE_CPU_FIRST === 'only';\n}\nexport function watchPermanentCpuCanvas(onPermanent) {\n  if (typeof PerformanceObserver === 'undefined') return;\n  const marks = new Set(['web.gpu.failed', 'web.handover.failed', 'web.gpu.lost']);\n  const obs = new PerformanceObserver(list => {\n    for (const entry of list.getEntries()) {\n      if (marks.has(entry.name)) {\n        obs.disconnect();\n        onPermanent();\n        return;\n      }\n    }\n  });\n  try { obs.observe({ type: 'mark', buffered: true }); } catch {}\n}\nexport function watchKeyboardDismiss(onDismiss) {\n  let touchDown = 0;\n  window.addEventListener('touchstart', () => { touchDown++; }, { passive: true, capture: true });\n  const end = () => { touchDown = Math.max(0, touchDown - 1); };\n  window.addEventListener('touchend', end, { passive: true, capture: true });\n  window.addEventListener('touchcancel', end, { passive: true, capture: true });\n  window.addEventListener('focusout', ev => {\n    const tag = ev.target?.tagName;\n    if (tag !== 'TEXTAREA' && tag !== 'INPUT') return;\n    queueMicrotask(() => {\n      if (touchDown > 0) return;\n      const a = document.activeElement?.tagName;\n      if (a === 'TEXTAREA' || a === 'INPUT') return;\n      onDismiss();\n    });\n  });\n}\n")]
extern "C" {
    #[wasm_bindgen(js_name = initialPointer)]
    fn initial_pointer() -> u32;
    #[wasm_bindgen(js_name = initialCpuCanvasOnly)]
    fn initial_cpu_canvas_only() -> bool;
    #[wasm_bindgen(js_name = watchPermanentCpuCanvas)]
    fn watch_permanent_cpu_canvas(on_permanent: &js_sys::Function);
    #[wasm_bindgen(js_name = watchKeyboardDismiss)]
    fn watch_keyboard_dismiss(on_dismiss: &js_sys::Function);
}

/// The page calls this once the module is in, with the server's password if it has one. A JSPI
/// export, so the app's background work runs on fibers that wait on the network without holding
/// the page (`rune::task`). The password lives only in the kit's memory, which sends it as the
/// `Authorization` header: nothing stores or logs it.
#[allow(deprecated)]
#[wasm_bindgen(jspi)]
pub fn start(password: Option<String>) {
    rune::task::start_runners();
    let pointer = initial_pointer();
    session_view::style::set_pointer(pointer & 1 != 0, pointer & 2 != 0);
    let cpu_only = initial_cpu_canvas_only();
    session_view::style::set_permanent_cpu_canvas(cpu_only);
    // The dictionaries beside the page (`spell/`, scripts/web.ps1).
    rune::spell::set_web_dictionaries(&["en-US"]);
    // The page's arguments first: the storage and the app's spec read them.
    let args = page_args();
    rune::tooling::Args::parse_from(args.clone());
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        let server = web_api::arg("--server").unwrap_or_else(|| opencode_kit::client::DEFAULT_SERVER.to_owned());
        opencode_kit::local_service::remember_page_password(&server, &password);
    }
    // The app's state: the browser's storage, as the database is natively.
    web_api::init_features();
    web_api::storage_start();
    // Start the reads before the first view. If they are already in, setup can connect here.
    web_api::launch_defer();
    web_api::launch_start_first_reads();
    let app_spec = rune::AppSpec { families: web_api::launch_families(), poll: Some(poll), ..web_api::spec(web_api::new_app) };
    let app = web_api::LATE_FONTS.iter().fold(web_api::rune_app(app_spec).args(args), |app, url| app.font_url_later(url));
    let handle = app.handle();
    let wake = web_api::launch_inbox_waker(handle.clone());
    let i18n_wake = wake.clone();
    opencode_i18n::on_loaded(move || i18n_wake());
    browser_history::listen(handle.clone());
    sync_keyboard_dismiss(handle.clone());
    if !cpu_only {
        sync_permanent_cpu_canvas(handle.clone());
    }
    flush_on_pagehide();
    app.init(move || {
        let mut s = web_api::new_app();
        if let Some(dpr) = web_sys::window().map(|w| w.device_pixel_ratio() as f32).filter(|&v| v > 0.0) {
            web_api::set_window_scale(&mut s, dpr);
            session_view::scroller::set_device_scale(web_api::view_scale(&s));
        }
        web_api::set_inbox_wake(&mut s, wake);
        s
    }).on_ready(move |_| {
        web_api::storage_presented();
        handle.update_if(web_api::launch_ready);
    })
    .run();
}

/// The app's poll, then the browser's history catching up with where it went.
fn poll(s: &mut App, now: f64, tx: &mut rune::Tx) -> bool {
    if let Some(dpr) = web_sys::window().map(|w| w.device_pixel_ratio() as f32).filter(|&v| v > 0.0) {
        web_api::set_window_scale(s, dpr);
    }
    let changed = web_api::poll(s, now, tx);
    browser_history::sync(s);
    changed
}

/// On a phone or tablet, dismissing the OS soft keyboard blurs the hidden `<textarea>` without
/// touching the canvas: mirror that blur back into `App` so the next tap on a `preserve_focus`
/// button doesn't raise the keyboard again.
fn sync_keyboard_dismiss(handle: rune::AppHandle<App>) {
    let on_dismiss = Closure::<dyn FnMut()>::new(move || {
        handle.update_if(|s, _| {
            if !session_view::style::touch_controls() {
                return false;
            }
            web_api::dismiss_touch_keyboard(s)
        });
    });
    watch_keyboard_dismiss(on_dismiss.as_ref().unchecked_ref());
    on_dismiss.forget();
}

/// When WebGPU fails or loses the device (`web.gpu.failed`, `web.handover.failed`, `web.gpu.lost`),
/// the page stays on `CpuCanvas` for good: switch `backdrop_blur` surfaces to their flat tint.
fn sync_permanent_cpu_canvas(handle: rune::AppHandle<App>) {
    let on_permanent = Closure::<dyn FnMut()>::new(move || {
        handle.update_if(|_, _| {
            if session_view::style::permanent_cpu_canvas() {
                return false;
            }
            session_view::style::set_permanent_cpu_canvas(true);
            true
        });
    });
    watch_permanent_cpu_canvas(on_permanent.as_ref().unchecked_ref());
    on_permanent.forget();
}

/// Closing or reloading a browser tab never runs winit's `on_close`: land any queued terminal
/// snapshot and write-behind state rows synchronously on `pagehide` so nothing in the 1 s / 250 ms
/// buffers is lost.
fn flush_on_pagehide() {
    let Some(window) = web_sys::window() else { return };
    let on_hide = Closure::<dyn FnMut()>::new(|| {
        web_api::terminal_flush();
        web_api::storage_flush(opencode_storage::FlushReason::Close, false);
    });
    let _ = window.add_event_listener_with_callback("pagehide", on_hide.as_ref().unchecked_ref());
    on_hide.forget();
}

/// Known, validated page query parameters mapped to app arguments. CLI-only, headless and
/// filesystem flags (`?help`, `?scale=…`, `?scene=…`, `?fixture=…`, `?snap=…`), malformed values
/// and arbitrary link parameters (`?utm_source=…`) are ignored rather than trapping before the
/// first frame.
fn page_args() -> Vec<String> {
    let Some(search) = web_sys::window().and_then(|w| w.location().search().ok()) else { return Vec::new() };
    let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in params.entries().into_iter().flatten() {
        let pair = js_sys::Array::from(&entry);
        let (Some(key), value) = (pair.get(0).as_string(), pair.get(1).as_string().unwrap_or_default()) else { continue };
        match key.as_str() {
            "server" | "session" | "directory" | "language" | "tab-layout" | "preset" | "ui-font" | "code-font" | "font-smoothing" => {
                if !value.is_empty() {
                    out.push(format!("--{key}"));
                    out.push(value);
                }
            }
            "theme" if matches!(value.as_str(), "light" | "dark") => {
                out.push("--theme".into());
                out.push(value);
            }
            "scroll-bars" if matches!(value.as_str(), "when-scrolling" | "always" | "automatic") => {
                out.push("--scroll-bars".into());
                out.push(value);
            }
            "zoom" if value.parse::<f32>().is_ok_and(f32::is_finite) => {
                out.push("--zoom".into());
                out.push(value);
            }
            "egg-seed" if value.parse::<u32>().is_ok() => {
                out.push("--egg-seed".into());
                out.push(value);
            }
            "debug" | "reduced-motion" | "sidebar-closed" | "read-only" | "no-plugins" | "plugins-eager" | "no-stream" => {
                if !matches!(value.as_str(), "0" | "false") {
                    out.push(format!("--{key}"));
                }
            }
            _ => {}
        }
    }
    out
}
