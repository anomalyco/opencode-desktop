//! A single status window per attempt. A stage change posts one update; no timer drives
//! the surface, including when a sync has stopped making progress.
use std::sync::Mutex;

use opencode_i18n::t;
use rune::{AppHandle, AppSpec, WindowSpec, col, el, text};

use super::{Stage, Ticket, stage};

type View = Option<(Ticket, Stage)>;
static VIEW: Mutex<Option<AppHandle<View>>> = Mutex::new(None);
thread_local! { static SURFACE: std::cell::RefCell<Option<rune::WindowHandle>> = const { std::cell::RefCell::new(None) }; }

pub(super) fn dismiss() {
    VIEW.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(window) = SURFACE.with_borrow_mut(Option::take) { window.close(); }
}

pub(super) fn refresh(ticket: Ticket) {
    let handle = VIEW.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(view) = handle {
        view.update_if(move |shown, _| {
            let Some((k, previous)) = shown else { return false };
            if *k != ticket { return false; }
            let Some(current) = stage(ticket) else { return false };
            if current == *previous { return false; }
            *previous = current;
            true
        });
    }
}

/// The surface's X or Alt+F4: while nothing is being saved (Changes won't be saved), it means
/// Return to workspace, the default; while a save runs it means Keep waiting.
pub(super) fn closed(k: Ticket) {
    if matches!(stage(k), Some(Stage::Volatile(_))) { k.return_to_workspace(); }
}

/// The window's keys, focused or not: Escape is Return to workspace while nothing is being saved,
/// and Keep waiting (nothing) while a save runs.
fn escape(state: &mut View, ev: &rune::KeyEvent, _: &mut rune::Tx) -> bool {
    match state {
        Some((k, Stage::Volatile(_))) if ev.key == rune::Key::Escape => { k.return_to_workspace(); true }
        _ => false,
    }
}

pub(super) fn overdue(k: Ticket) -> rune::WindowHandle {
    let initial = stage(k).unwrap_or(Stage::Failed);
    if let Some(existing) = SURFACE.with_borrow(Clone::clone) { refresh(k); return existing; }
    let title=if matches!(initial,Stage::Volatile(_)){t!("rune.storage.memoryOnly.title")}else{t!("rune.storage.quit.title")};
    let app = rune::App::new(AppSpec { on_key: Some(escape), ..AppSpec::new(title, (530., 310.), || None, |state: &View, cx| {
        let Some((k, shown)) = state.clone() else { return el() };
        let p=crate::palette::pal(cx.system_dark());
        let panel=col().w_full().h_full().px(20.).py(18.).gap(10.).fill(p.bg_base).dir(crate::view::ui_direction());
        if let Stage::Volatile(r)=shown {
            let choice=r.clone();
            // A question with a safe default: Escape (scape) and the window's X return to it.
            let panel=panel.role("alertdialog").label(t!("rune.storage.memoryOnly.title"));
            return panel.child(text(t!("rune.storage.memoryOnly.title")).color(p.text).wrap().w_full()).child(text(r.reason.clone()).color(p.text).wrap().w_full())
                .child(text(r.drafts.clone()).color(p.text).wrap().w_full())
                .child(el().key("return-workspace").role("button").focusable(true).autofocus().label(t!("rune.storage.quit.return")).px(10.).py(6.).fill(p.layer2).child(text(t!("rune.storage.quit.return")).color(p.text)).on_press(move |_:&mut View|k.return_to_workspace()))
                .child(el().key("discard-workspace").role("button").focusable(true).label(t!("rune.storage.quit.discard")).px(10.).py(6.).child(text(t!("rune.storage.quit.discard")).color(p.text)).on_press(move |_:&mut View|super::send(super::Event::Discard(k,choice.clone()))));
        }
        let why = match &shown {
            Stage::Windows => t!("rune.storage.quit.producers"),
            Stage::Database => t!("rune.storage.quit.database"),
            Stage::Failed => t!("rune.storage.quit.failed"),
            Stage::Volatile(_) => unreachable!(),
        };
        // A warning while a save runs; Escape, like the X, keeps waiting.
        panel.role("alertdialog").label(t!("rune.storage.quit.title")).child(text(t!("rune.storage.quit.title")).color(p.text).wrap().w_full()).child(text(why).color(p.text).wrap().w_full()).child(text(t!("rune.storage.quit.risk")).color(p.text).wrap().w_full())
            .child(el().key("keep-waiting").role("button").label(t!("rune.storage.quit.wait")).focusable(true).autofocus()
                .px(10.).py(6.).fill(p.layer2).child(text(t!("rune.storage.quit.wait")).color(p.text)).on_press(|_: &mut View| {}))
            .child(el().key("quit-anyway").role("button").label(t!("rune.storage.quit.force")).focusable(true)
                .px(10.).py(6.).child(text(t!("rune.storage.quit.force")).color(p.text)).on_press(move |s: &mut View| match stage(k) {
                    Some(now) if now == shown => k.quit_anyway(),
                    Some(now) => *s = Some((k, now)),
                    None => {}
                }))
    }) }).init(move || Some((k, initial))).args(Vec::new());
    *VIEW.lock().unwrap_or_else(|e| e.into_inner()) = Some(app.handle());
    let window = rune::open_window(WindowSpec::from_app(k.status_key(), app));
    SURFACE.with_borrow_mut(|slot| *slot = Some(window.clone()));
    refresh(k); // close the race between reading the initial stage and binding its view
    rune::marks::mark("save.overdue");
    window
}
