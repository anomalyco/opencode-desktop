//! Carrying out what plugins asked for ([`cx::ask`]) with `&mut App` and the transaction, right
//! after the entry that asked.

use std::cell::RefCell;

use opencode_kernel::AppFeedback;
use rune::Tx;
use rune_luau::Data;

use super::cx::{self, Request};
use crate::App;
use crate::plugin_host::panels::tab_key;
use crate::plugin_host::{FlyoutEvent, OpenDialog, Overlay, bump, services};

pub fn carry_out(app: &mut App, tx: &mut Tx) {
    let asked = cx::take_asked();
    if asked.is_empty() {
        return;
    }
    for (plugin, request) in asked {
        apply(app, &plugin, request, tx);
    }
    bump();
}

fn apply(app: &mut App, plugin: &std::rc::Rc<str>, request: Request, tx: &mut Tx) {
    match request {
        Request::OpenPanel { panel, instance, instant } => {
            if instant {
                tx.instant();
            }
            if app.active_session_id().is_some() {
                app.open_side_tab(&tab_key(plugin, &panel, instance.as_deref()), None, false, false, tx);
            }
        }
        Request::TogglePanel { panel, session, instant } => {
            if instant {
                tx.instant();
            }
            let Some(active) = app.active_session_id().map(str::to_owned) else { return };
            let session = session.unwrap_or(active);
            let key = tab_key(plugin, &panel, None);
            if services::ui_panel_state(app, plugin, &panel, &session) == 3 {
                app.close_side_tab(&key, tx);
            } else {
                app.open_side_tab(&key, None, false, false, tx);
            }
        }
        Request::ClosePanel { panel, .. } => app.close_side_tab(&tab_key(plugin, &panel, None), tx),
        Request::OpenPopover { id, anchor, trigger, side, align, gap, data, toggle } => {
            if toggle && app.plugins.overlay.popover_open(plugin, &id) {
                close_popover(app, tx);
                return;
            }
            tx.instant();
            // Where focus goes back as it closes: its trigger, as Kobalte's popover returns it
            // even when a shortcut opened it, else what had focus.
            let back = trigger.or_else(|| tx.focused().map(str::to_owned));
            tx.focus(format!("plugin-popover-{plugin}.{id}"), tx.focus_visible());
            app.plugins.overlay = Overlay::Popover { plugin: plugin.clone(), id, anchor, side, align, gap, data, flyout: Box::default(), back };
            crate::popover::motion::opening(&app.diag.knobs, tx);
        }
        Request::ClosePopover => close_popover(app, tx),
        Request::Flyout(event) => step_flyout(app, Some(plugin), event, tx),
        Request::OpenDialog { id, data } => {
            tx.instant();
            // A dialog takes over from a popover, as Kobalte's does as focus leaves it.
            close_popover(app, tx);
            app.plugins.dialog = Some(OpenDialog { plugin: plugin.clone(), id, data });
        }
        // Only the drawer kind view of this plugin that the More drawer shows opens one.
        Request::DrawerView { title, data } => {
            let Some(view) = app.layout.narrow.more.drawn_view().filter(|v| crate::panels::split_panel_key(v).is_some_and(|(p, _)| p == &**plugin)).map(str::to_owned) else { return };
            let data = if data == Data::Nil { Data::Map(Default::default()) } else { data };
            tx.instant();
            app.more_drawer(crate::narrow::machine::MoreEvent::Nest { view, nested: crate::narrow::machine::Nested { title, data: data.to_json() } }, tx);
        }
        Request::CloseDialog => {
            if app.plugins.dialog.take().is_some() {
                tx.instant();
            }
        }
        Request::DismissToast(handle) => {
            if let Some(id) = app.toast.id_for_handle(handle) {
                tx.instant();
                crate::status_toast::dismiss_id(app, id);
            }
        }
        Request::UpdateToast { handle, variant, title, description, reveal, persistent, buttons } => {
            let Some(entry) = app.toast.by_handle_mut(handle) else { return };
            entry.title = title;
            entry.description = Some(description.trim().to_owned()).filter(|d| !d.is_empty());
            entry.icon = (variant == "success").then_some(&rune_theme_opencode::icons::CIRCLE_CHECK);
            entry.persistent = persistent || variant == "error";
            entry.action = reveal.map(|(label, path)| crate::status_toast::ToastAction::Reveal(label, path.into()));
            entry.buttons = crate::status_toast::plugin_buttons(buttons);
            tx.instant();
        }
        Request::OpenIn { app: id, path } => app.open_in_at(&id, &path),
        Request::Toast { variant, title, description, reveal, persistent, buttons, handle } => {
            match variant.as_str() {
                // A persistent toast stays until dismissed, as an error does.
                "error" => app.toast_fail(title, description),
                v if persistent && v != "loading" => app.toast_fail(title, description),
                "loading" => {
                    crate::status_toast::waiting(app, &title);
                }
                _ => app.toast_say(title, description),
            }
            if let Some(e) = app.toast.current_mut() {
                if variant == "success" {
                    e.icon = Some(&rune_theme_opencode::icons::CIRCLE_CHECK);
                }
                if let Some((label, path)) = reveal {
                    e.action = Some(crate::status_toast::ToastAction::Reveal(label, path.into()));
                }
                e.buttons = crate::status_toast::plugin_buttons(buttons);
                e.handle = Some(handle);
            }
        }
        Request::OpenSettings(page) => {
            tx.instant();
            app.open_settings();
            match page.as_deref() {
                Some("servers") => app.open_settings_page(crate::settings::pages::Page::Servers, tx),
                Some(page) if !page.is_empty() => app.open_settings_page(crate::settings::pages::Page::Plugin(format!("{plugin}.{page}")), tx),
                _ => {}
            }
        }
        Request::NewDraft { directory, prompt } => {
            tx.instant();
            if !directory.is_empty() {
                *app.default_dir = directory;
            }
            app.close_settings();
            app.open_draft_tab();
            app.composer.replace_text(prompt);
            tx.settle();
        }
        Request::OpenDraft { server, directory, prompt } => {
            let key = match server.as_deref() {
                None => crate::servers::runtime::active(app).clone(),
                Some(opencode_kit::servers::ServerScope::LOCAL) => opencode_kit::servers::ServerKey::sidecar(),
                Some(key) => opencode_kit::servers::ServerKey::new(key),
            };
            if !crate::servers::runtime::listed().iter().any(|l| l.key == key) {
                return;
            }
            tx.instant();
            app.close_settings();
            crate::servers::runtime::switch(app, &key);
            if !directory.is_empty() {
                *app.default_dir = directory;
            }
            app.open_draft_tab();
            app.composer.replace_text(prompt);
            tx.settle();
        }
        Request::OpenProject { server, title } => super::project_request::request(app, server, title, tx),
        Request::RunCommand { id, input } => match (id.strip_prefix("server:"), input) {
            (Some(key), _) => crate::plugin_host::connect_server(app, key, tx),
            (None, Some(input)) => {
                crate::plugin_host::run_slash(app, &id, &input, tx);
            }
            (None, None) => app.run_command(&id, tx),
        },
        Request::OpenLink { href } => app.open_reply_link(&href, tx),
        Request::OpenExternal(url) => {
            if app.live() {
                crate::open_in::open_external(tx, &url);
            }
        }
        Request::Reveal(path) => crate::plugin_host::reveal(&path),
        Request::CopyText(text) => tx.copy(text),
        // A muted session, or one under a muted root, shows nothing (`host-apis.tsx`'s notify).
        Request::Notify { session: Some(id), .. } if crate::session_menu::preferences::silenced(app, crate::servers::runtime::active(app), &id) => {}
        Request::Notify { title, body, .. } => app.toast_say(title, body),
        Request::Menu { anchor, menu, done } => {
            let done = RefCell::new(Some(done));
            crate::popup::open(app, menu, anchor, tx, move |_: &mut App, picked: Option<&str>, _: &mut Tx| {
                if let Some(done) = done.borrow_mut().take() {
                    done.complete(picked.map_or(Data::Nil, |p| Data::String(p.to_owned())));
                }
            });
        }
        Request::Dev { name, value } => services::diag::apply(app, &name, value, tx),
        Request::ServersChanged => crate::plugin_host::servers_changed(app, tx),
        Request::Redraw => {}
    }
}

/// Steps the open popover's flyout (of `plugin`, when named) with `event`.
pub fn step_flyout(app: &mut App, plugin: Option<&std::rc::Rc<str>>, event: FlyoutEvent, tx: &mut Tx) {
    let clock = *app.clock;
    let Overlay::Popover { plugin: owner, flyout, .. } = &mut app.plugins.overlay else { return };
    if plugin.is_some_and(|p| p != owner) {
        return;
    }
    let (next, effect) = std::mem::take(&mut **flyout).step(event);
    **flyout = next;
    if effect.changed {
        // Lists swap at once, as Desktop's do without the submenu motion knob.
        tx.instant();
    }
    if let Some(at) = effect.wake_at {
        tx.wake_after(std::time::Duration::from_secs_f64((at - clock).max(0.0)));
    }
}

/// Esc or a press outside closed the open dialog: its `onDismiss` runs.
pub fn dismiss_dialog(app: &mut App, tx: &mut Tx) {
    let Some(open) = app.plugins.dialog.take() else { return };
    tx.instant();
    let found = app.plugins.running().find_map(|(i, p)| (p.half.id == open.plugin).then(|| p.half.contribs.borrow().dialogs.iter().find(|c| c.spec.id == open.id).and_then(|c| c.f("onDismiss").cloned()).map(|f| (i, f))).flatten());
    if let Some((i, f)) = found {
        super::commands::spawn(app, i, &f, (), tx);
    }
}

pub fn close_popover(app: &mut App, tx: &mut Tx) {
    let closed = std::mem::take(&mut app.plugins.overlay);
    if closed.is_open() {
        tx.instant();
        crate::popover::motion::closing(&app.diag.knobs, tx);
    }
    // Focus inside it goes back where it came from (`Tx::return_focus`: a press elsewhere that
    // focused something keeps it).
    if let Overlay::Popover { back: Some(back), .. } = closed {
        tx.return_focus(back, tx.focus_visible());
    }
}
