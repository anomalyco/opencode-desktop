//! Native Rune UI for browser prompt bars (blocked pop-ups, site permissions) and in-pane modal
//! dialogs (JS alert/confirm/prompt, "Leave site?", HTTP auth), matching official `e51247f626`
//! (`packages/gui-plugins/src/browser/prompts.tsx`).

use browser_pane::prompts::DialogGate;
use browser_pane::{DialogKind, PermissionDecision, PermissionKind};
use opencode_i18n::t;
use rune::{Cursor, El, KeyEvent, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, TextInput};
use rune_theme_opencode::icons as ic;
use session_view::palette::Elevate;

use super::keys::handle_modal_key;
use super::{
    AUTH_CANCEL_KEY, AUTH_CARD_KEY, AUTH_PASS_KEY, AUTH_SUBMIT_KEY, AUTH_USER_KEY,
    BROWSER_PROMPTS, PROMPT_CANCEL_KEY, PROMPT_CARD_KEY, PROMPT_INPUT_KEY, PROMPT_OK_KEY,
    PROMPT_STOP_KEY, clear_auth_fields,
};
use crate::App;
use crate::palette::{Pal, kit};
use crate::view::{UI, UI_MED, UI_SMALL};

/// Official's notice copy (`refusal-bar.tsx`): 13 px regular on 24 px lines.
const NOTICE: rune::TextStyle = rune::TextStyle::new(13.0, 440.0).lh(24.0);
/// The notice's destination row (`text-12-regular`): 12 px on 20 px lines, muted.
const NOTICE_TARGET: rune::TextStyle = rune::TextStyle::new(12.0, 440.0).lh(20.0);

/// Top-of-viewport strips (blocked pop-up notice, site permission bar) rendered above the page.
pub fn view_bars(s: &App, p: &Pal, tab_id: u64) -> Vec<El> {
    let Some(tab) = s.workspace_panes.browser.host.tab(tab_id) else { return Vec::new() };
    let mut out = Vec::new();
    let k = kit(p);

    if let Some(reason) = tab.prompts.blocked_guard_notice.as_ref()
        && (tab.prompts.agent_driven || matches!(reason, browser_pane::BlockedGuardReason::Passkey { .. }))
    {
        let plugin_name = match tab.prompts.driver.as_ref() {
            Some(browser_pane::TabDriver::Plugin(name)) => Some(name.as_str()),
            _ => None,
        };
        let (notice, reopen_url, import_signin) = match reason {
            browser_pane::BlockedGuardReason::InheritedPermission { url } => (
                match plugin_name {
                    Some(name) => t!("rune.browser.prompt.controlled.inheritedPermission.plugin", name = name),
                    None => t!("rune.browser.prompt.controlled.inheritedPermission.agent").to_owned(),
                },
                url.clone().unwrap_or_else(|| tab.url.clone()),
                false,
            ),
            browser_pane::BlockedGuardReason::Passkey { native: true, url } => (
                t!("rune.browser.prompt.passkey.off").to_owned(),
                url.clone().unwrap_or_else(|| tab.url.clone()),
                false,
            ),
            browser_pane::BlockedGuardReason::Passkey { native: false, .. } => (
                t!("rune.browser.prompt.passkey").to_owned(),
                String::new(),
                true,
            ),
            browser_pane::BlockedGuardReason::Action => (
                match plugin_name {
                    Some(name) => t!("rune.browser.prompt.controlled.plugin", name = name),
                    None => t!("rune.browser.prompt.controlled.agent").to_owned(),
                },
                tab.url.clone(),
                false,
            ),
        };
        let can_reopen = !import_signin
            && !reopen_url.is_empty()
            && !reopen_url.eq_ignore_ascii_case("about:blank")
            && tab.preview.is_none()
            && !matches!(
                tab.context,
                browser_pane::ContextKind::Preview { .. } | browser_pane::ContextKind::Artifact { .. }
            )
            && tab.refusal.as_deref() != Some("browser.proxy.required");
        let mut actions = row().gap(4.0).center_y();
        if import_signin {
            #[allow(deprecated)]
            let import_btn = crate::widgets::button(t!("rune.browser.prompt.passkey.import"), ButtonVariant::Neutral)
                .size(rune_kit::ButtonSize::Small)
                .on_press(move |app: &mut App| {
                    if !app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.blocked_guard_armed()) {
                        return;
                    }
                    app.workspace_panes.browser.host.dismiss_blocked_guard_notice(tab_id);
                    app.open_settings_at(crate::settings::pages::Page::Plugin("browser.browser".to_owned()));
                })
                .build(k)
                .key("browser-passkey-import");
            actions = actions.child(import_btn);
        }
        if can_reopen {
            #[allow(deprecated)]
            let reopen_btn = crate::widgets::button(
                t!("rune.browser.prompt.controlled.openNewTab"),
                ButtonVariant::Neutral,
            )
            .size(rune_kit::ButtonSize::Small)
            .on_press(move |app: &mut App| {
                if !app
                    .workspace_panes
                    .browser
                    .host
                    .tab(tab_id)
                    .is_some_and(|t| t.prompts.blocked_guard_armed())
                {
                    return;
                }
                if let Some((_, mut ctx)) = app.browser_context_for_opener(tab_id) {
                    let url = reopen_url.clone();
                    let proxy_ok = match &mut ctx {
                        browser_pane::ContextKind::RemoteServer { server_id, proxy } => {
                            *proxy = app.workspace_panes.browser.host.remote_proxy(server_id.trim()).cloned();
                            proxy.is_some()
                        }
                        _ => true,
                    };
                    if proxy_ok && !url.is_empty() && !url.eq_ignore_ascii_case("about:blank") {
                        let sid = app.browser_tab_session(tab_id).map(str::to_owned);
                        app.open_new_browser_tab_with_context(sid.as_deref(), &url, None, true, false, ctx, None);
                        app.workspace_panes.browser.host.dismiss_blocked_guard_notice(tab_id);
                    }
                }
            })
            .build(k)
            .key("browser-controlled-open-new");
            actions = actions.child(reopen_btn);
        }
        let close_btn = dismiss_icon_btn(p, "browser-controlled-dismiss", t!("common.close"), move |app: &mut App| {
            app.workspace_panes.browser.host.dismiss_blocked_guard_notice(tab_id);
        });
        actions = actions.child(close_btn);
        out.push(
            col()
                .key("browser-controlled-bar")
                .w_full()
                .child(
                    row()
                        .role("status")
                        .w_full()
                        .min_h(36.0)
                        .px(12.0)
                        .py(8.0)
                        .gap(12.0)
                        .center_y()
                        .fill(p.layer1)
                        .child(icon(&ic::WARNING, 14.0).color(p.icon_muted))
                        .child(text(notice).font(UI_SMALL).color(p.text).grow(1.0).shrink().min_w(0.0).wrap())
                        .child(actions),
                )
                .child(el().w_full().h(1.0).fill(p.border_muted)),
        );
    }

    if let Some((url, count, _armed)) = tab.prompts.popup.blocked() {
        let isolated = tab.preview.is_some() || tab.context.is_isolated();
        // Official's notice (`refusal-bar.tsx`): the opener's origin and the refusal stacked in one
        // cell beside the buttons, the destination on a muted row of its own under all three.
        let opener = match browser_pane::policy::destination_origin(&tab.url) {
            Some(site) if site.starts_with("http://") || site.starts_with("https://") => {
                t!("rune.browser.prompt.popup.fromSite", site = site)
            }
            _ => t!("rune.browser.prompt.popup.fromPage").to_owned(),
        };
        let msg = if count > 1 {
            format!("{} ({count})", t!("rune.browser.prompt.popup.blocked"))
        } else {
            t!("rune.browser.prompt.popup.blocked").to_owned()
        };
        let target = popup_target(p, url, isolated);
        #[allow(deprecated)]
        let open_btn = crate::widgets::button(t!("rune.browser.prompt.popup.open"), ButtonVariant::Neutral)
            .size(rune_kit::ButtonSize::Small)
            .on_press(move |app: &mut App| {
                // `take_blocked_popup_url` enforces the 500 ms activation delay.
                if let Some(url) = app.workspace_panes.browser.host.take_blocked_popup_url(tab_id)
                    && url.len() <= 16_384
                    && browser_pane::policy::destination_origin(&url).is_some()
                    && let Some((driver, ctx)) = app.browser_context_for_opener(tab_id)
                {
                    let sid = app.browser_tab_session(tab_id).map(str::to_owned);
                    app.open_new_browser_tab_with_context(sid.as_deref(), &url, None, true, false, ctx, driver);
                }
            })
            .build(k)
            .key("browser-popup-open");
        let close_btn = sized_dismiss_btn(p, "browser-popup-dismiss", t!("common.close"), 20.0, 4.0, move |app: &mut App| {
            app.workspace_panes.browser.host.dismiss_blocked_popup(tab_id);
        });
        let lines = col()
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .child(text(opener).font(NOTICE).color(p.text).wrap_anywhere())
            .child(text(msg).font(NOTICE).color(p.text).wrap_anywhere());
        out.push(
            col()
                .key("browser-popup-bar")
                .w_full()
                .child(
                    col()
                        .role("status")
                        .w_full()
                        .px(12.0)
                        .py(8.0)
                        .fill(p.layer1)
                        .child(row().w_full().gap(12.0).center_y().child(lines).child(open_btn).child(close_btn))
                        .children(target),
                )
                .child(el().w_full().h(1.0).fill(p.border_muted)),
        );
    }

    if let Some(queued) = tab.prompts.permission.active() {
        let req_id = queued.request.id;
        let agent = queued.agent_driven;
        // RemoteServer, Private, Artifact, Preview, and agent-driven tabs are strictly visit-only
        // (never offer "Always allow" / "Never allow").
        let can_remember = !agent
            && tab.preview.is_none()
            && tab.context.is_default()
            && !queued.request.origin.to_ascii_lowercase().starts_with("oc-preview:");
        let waiting = tab.prompts.permission.waiting_count();
        let sentence = permission_sentence(&queued.request.origin, &queued.request.kinds);

        let mut msg_col = col()
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .gap(2.0)
            .child(text(sentence).font(UI).color(p.text).wrap());
        if agent {
            let driver_copy = match tab.prompts.driver.as_ref() {
                Some(browser_pane::TabDriver::Plugin(name)) => {
                    t!("rune.browser.prompt.perm.plugin", name = name.as_str())
                }
                _ => t!("rune.browser.prompt.perm.agent").to_owned(),
            };
            msg_col = msg_col.child(
                text(driver_copy)
                    .font(UI_MED)
                    .color(p.text)
                    .wrap(),
            );
        }
        if waiting > 0 {
            msg_col = msg_col.child(
                text(t!("rune.browser.prompt.waiting", count = waiting))
                    .font(UI_SMALL)
                    .color(p.muted),
            );
        }

        #[allow(deprecated)]
        let mut actions = row().gap(4.0).center_y();
        if can_remember {
            #[allow(deprecated)]
            let never = crate::widgets::button(t!("rune.browser.prompt.never"), ButtonVariant::Ghost)
                .size(rune_kit::ButtonSize::Small)
                .on_press(move |app: &mut App| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.permission.armed()) {
                        app.workspace_panes.browser.host.answer_permission(tab_id, req_id, PermissionDecision::BlockAlways);
                    }
                })
                .build(k)
                .key("browser-perm-never");
            #[allow(deprecated)]
            let once = crate::widgets::button(t!("rune.browser.prompt.allowOnce"), ButtonVariant::Neutral)
                .size(rune_kit::ButtonSize::Small)
                .on_press(move |app: &mut App| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.permission.armed()) {
                        app.workspace_panes.browser.host.answer_permission(tab_id, req_id, PermissionDecision::AllowOnce);
                    }
                })
                .build(k)
                .key("browser-perm-allow-once");
            #[allow(deprecated)]
            let always = crate::widgets::button(t!("rune.browser.prompt.allowAlways"), ButtonVariant::Contrast)
                .size(rune_kit::ButtonSize::Small)
                .on_press(move |app: &mut App| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.permission.armed()) {
                        app.workspace_panes.browser.host.answer_permission(tab_id, req_id, PermissionDecision::AllowAlways);
                    }
                })
                .build(k)
                .key("browser-perm-allow-always");
            actions = actions.child(never).child(once).child(always);
        } else {
            #[allow(deprecated)]
            let block = crate::widgets::button(t!("rune.browser.prompt.block"), ButtonVariant::Ghost)
                .size(rune_kit::ButtonSize::Small)
                .on_press(move |app: &mut App| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.permission.armed()) {
                        app.workspace_panes.browser.host.answer_permission(tab_id, req_id, PermissionDecision::BlockOnce);
                    }
                })
                .build(k)
                .key("browser-perm-block");
            #[allow(deprecated)]
            let once = crate::widgets::button(t!("rune.browser.prompt.allowOnce"), ButtonVariant::Contrast)
                .size(rune_kit::ButtonSize::Small)
                .on_press(move |app: &mut App| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.permission.armed()) {
                        app.workspace_panes.browser.host.answer_permission(tab_id, req_id, PermissionDecision::AllowOnce);
                    }
                })
                .build(k)
                .key("browser-perm-allow-once");
            actions = actions.child(block).child(once);
        }
        // Official's dismiss is `IconButton size="large"` (28 pt, `prompts.tsx:557-563`).
        let close_btn = sized_dismiss_btn(p, "browser-perm-dismiss", t!("rune.browser.prompt.dismiss"), 28.0, 6.0, move |app: &mut App| {
            app.workspace_panes
                .browser
                .host
                .answer_permission(tab_id, req_id, PermissionDecision::BlockOnce);
        });
        actions = actions.child(close_btn);

        out.push(
            col()
                .key("browser-perm-bar")
                .w_full()
                .child(
                    row()
                        .role("alertdialog")
                        .label_static(t!("rune.browser.prompt.perm.label"))
                        .w_full()
                        .min_h(40.0)
                        .px(12.0)
                        .py(8.0)
                        .gap(12.0)
                        .center_y()
                        .fill(p.layer1)
                        .child(icon(&ic::GLOBE, 16.0).color(p.icon_muted))
                        .child(msg_col)
                        .child(actions),
                )
                .child(el().w_full().h(1.0).fill(p.border_muted)),
        );
    }

    out
}

/// Centered in-pane modal (JS dialog or HTTP auth) rendered over the page surface.
pub fn view_modal(s: &App, p: &Pal, tab_id: u64) -> Option<El> {
    let tab = s.workspace_panes.browser.host.tab(tab_id)?;
    let k = kit(p);
    let ui = s.kernel.slots.get(&BROWSER_PROMPTS);

    if let DialogGate::Open { dialog, queue, repeat, .. } = &tab.prompts.dialog {
        let dialog_id = dialog.id;
        let kind = dialog.kind;
        let leave = kind == DialogKind::Leave;
        let waiting = queue.len();
        let title = if leave {
            t!("rune.browser.prompt.leave.title").to_owned()
        } else {
            prompt_site(&dialog.origin, &tab.url)
        };
        let message = if leave {
            t!("rune.browser.prompt.leave.description").to_owned()
        } else {
            dialog.message.clone()
        };

        let mut body = col()
            .w_full()
            .gap(12.0)
            .child(text(title.clone()).font(UI_MED).color(p.text).wrap());
        if !message.is_empty() {
            if leave {
                body = body.child(text(message).font(crate::widgets::BODY).color(p.muted).wrap());
            } else {
                // Scrollable bounded page text (`prompts.tsx:434`: `max-h-[160px] overflow-y-auto`).
                body = body.child(session_view::scroller::scroller("browser-prompt-msg").build::<App>(
                    p,
                    col().w_full().max_h(160.0),
                    text(message)
                        .font(crate::widgets::BODY)
                        .color(p.text)
                        .wrap_anywhere()
                        .selectable(true)
                        .selection_style(p.field_selection),
                ));
            }
        }
        if kind == DialogKind::Prompt {
            let field = TextInput::new(&ui.prompt_input)
                .width(None)
                .placeholder(t!("rune.browser.prompt.value").to_owned())
                .label(t!("rune.browser.prompt.value").to_owned())
                .on_edit(|app: &mut App| &mut app.kernel.slots.get_mut(&BROWSER_PROMPTS).prompt_input)
                .on_submit(move |app: &mut App, _tx: &mut Tx| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.dialog.armed()) {
                        let st = app.kernel.slots.get(&BROWSER_PROMPTS);
                        let val = st.prompt_input.text.clone();
                        let stop = st.stop_dialogs;
                        app.workspace_panes.browser.host.answer_dialog(tab_id, dialog_id, true, val, stop);
                    }
                })
                .build(k)
                .key(PROMPT_INPUT_KEY);
            body = body.child(field);
        }

        let mut start = row().gap(8.0).center_y().grow(1.0).shrink().min_w(0.0);
        if *repeat && !leave {
            let checked = ui.stop_dialogs;
            let toggle = row()
                .key(PROMPT_STOP_KEY)
                .role("checkbox")
                .focusable(true)
                .label_static(t!("rune.browser.prompt.stop"))
                .gap(8.0)
                .center_y()
                .cursor(Cursor::Pointer)
                .on_press(|app: &mut App| {
                    let st = app.kernel.slots.get_mut(&BROWSER_PROMPTS);
                    st.stop_dialogs = !st.stop_dialogs;
                })
                .child(
                    el().w(14.0)
                        .h(14.0)
                        .radius(3.0)
                        .border(1.0, if checked { p.q.submit } else { p.border })
                        .fill(if checked { p.q.submit } else { p.bg_base })
                        .center()
                        .children(checked.then(|| icon(&ic::CHECK, 10.0).color(p.q.submit_text))),
                )
                .child(text(t!("rune.browser.prompt.stop")).font(UI_SMALL).color(p.muted).wrap().grow(1.0).shrink().min_w(0.0));
            start = start.child(toggle);
        }
        if waiting > 0 {
            start = start.child(
                text(t!("rune.browser.prompt.waiting", count = waiting))
                    .font(UI_SMALL)
                    .color(p.muted),
            );
        }

        let mut footer = row().w_full().gap(8.0).pt(4.0).center_y().child(start);
        if kind != DialogKind::Alert {
            let cancel_label = if leave { t!("rune.browser.prompt.leave.stay") } else { t!("rune.browser.prompt.cancel") };
            footer = footer.child(
                crate::widgets::button(cancel_label, ButtonVariant::Ghost)
                    .on_press(move |app: &mut App| {
                        if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.dialog.armed()) {
                            let stop = app.kernel.slots.get(&BROWSER_PROMPTS).stop_dialogs;
                            app.workspace_panes.browser.host.answer_dialog(tab_id, dialog_id, false, String::new(), stop);
                        }
                    })
                    .build(k)
                    .key(PROMPT_CANCEL_KEY),
            );
        }
        let ok_label = if leave { t!("rune.browser.prompt.leave.leave") } else { t!("rune.browser.prompt.ok") };
        footer = footer.child(
            crate::widgets::button(ok_label, ButtonVariant::Contrast)
                .on_press(move |app: &mut App| {
                    if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.dialog.armed()) {
                        let st = app.kernel.slots.get(&BROWSER_PROMPTS);
                        let val = if kind == DialogKind::Prompt { st.prompt_input.text.clone() } else { String::new() };
                        let stop = st.stop_dialogs;
                        app.workspace_panes.browser.host.answer_dialog(tab_id, dialog_id, true, val, stop);
                    }
                })
                .build(k)
                .key(PROMPT_OK_KEY),
        );
        body = body.child(footer);

        let refocus = match kind {
            DialogKind::Prompt => PROMPT_INPUT_KEY,
            DialogKind::Alert => PROMPT_CARD_KEY,
            DialogKind::Confirm | DialogKind::Leave => PROMPT_CANCEL_KEY,
        };
        return Some(modal_frame(p, tab_id, PROMPT_CARD_KEY, refocus, title, body));
    }

    if let Some(queued) = tab.prompts.auth.active() {
        let auth_id = queued.challenge.id;
        let failed = queued.attempt > 1;
        let waiting = tab.prompts.auth.waiting_count();
        let origin_host = short_host(&queued.challenge.origin);
        let host = if origin_host.is_empty() {
            queued.challenge.host.clone()
        } else {
            origin_host
        };
        let title = t!("rune.browser.prompt.auth.title", host = host);
        let mut body = col()
            .w_full()
            .gap(12.0)
            .child(text(title.clone()).font(UI_MED).color(p.text).wrap());
        if failed {
            body = body.child(
                text(t!("rune.browser.prompt.auth.failed"))
                    .font(crate::widgets::BODY)
                    .color(p.danger)
                    .wrap(),
            );
        }
        if !queued.challenge.realm.is_empty() {
            // First-strong isolates keep RTL realms from reordering surrounding copy (`prompts.tsx:335`).
            let isolated = format!("\u{2068}{}\u{2069}", queued.challenge.realm);
            body = body.child(
                text(t!("rune.browser.prompt.auth.realm", realm = isolated))
                    .font(crate::widgets::BODY)
                    .color(p.muted)
                    .wrap_anywhere(),
            );
        }
        let user_field = TextInput::new(&ui.auth_user)
            .width(None)
            .placeholder(t!("rune.browser.prompt.auth.username").to_owned())
            .label(t!("rune.browser.prompt.auth.username").to_owned())
            .on_edit(|app: &mut App| &mut app.kernel.slots.get_mut(&BROWSER_PROMPTS).auth_user)
            .on_submit(|_app: &mut App, tx: &mut Tx| tx.focus(AUTH_PASS_KEY, true))
            .build(k)
            .key(AUTH_USER_KEY);
        let pass_field = TextInput::new(&ui.auth_pass)
            .secure()
            .width(None)
            .placeholder(t!("rune.browser.prompt.auth.password").to_owned())
            .label(t!("rune.browser.prompt.auth.password").to_owned())
            .on_edit(|app: &mut App| &mut app.kernel.slots.get_mut(&BROWSER_PROMPTS).auth_pass)
            .on_submit(move |app: &mut App, _tx: &mut Tx| {
                if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.auth.armed()) {
                    let st = app.kernel.slots.get(&BROWSER_PROMPTS);
                    let creds = (st.auth_user.text.clone(), st.auth_pass.text.clone());
                    clear_auth_fields(app);
                    app.workspace_panes.browser.host.answer_auth(tab_id, auth_id, Some(creds));
                }
            })
            .build(k)
            .key(AUTH_PASS_KEY);

        let mut start = row().gap(8.0).center_y().grow(1.0).shrink().min_w(0.0);
        if waiting > 0 {
            start = start.child(
                text(t!("rune.browser.prompt.waiting", count = waiting))
                    .font(UI_SMALL)
                    .color(p.muted),
            );
        }
        let footer = row()
            .w_full()
            .gap(8.0)
            .pt(4.0)
            .center_y()
            .child(start)
            .child(
                crate::widgets::button(t!("rune.browser.prompt.cancel"), ButtonVariant::Ghost)
                    .on_press(move |app: &mut App| {
                        if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.auth.armed()) {
                            clear_auth_fields(app);
                            app.workspace_panes.browser.host.answer_auth(tab_id, auth_id, None);
                        }
                    })
                    .build(k)
                    .key(AUTH_CANCEL_KEY),
            )
            .child(
                crate::widgets::button(t!("rune.browser.prompt.auth.signIn"), ButtonVariant::Contrast)
                    .on_press(move |app: &mut App| {
                        if app.workspace_panes.browser.host.tab(tab_id).is_some_and(|t| t.prompts.auth.armed()) {
                            let st = app.kernel.slots.get(&BROWSER_PROMPTS);
                            let creds = (st.auth_user.text.clone(), st.auth_pass.text.clone());
                            clear_auth_fields(app);
                            app.workspace_panes.browser.host.answer_auth(tab_id, auth_id, Some(creds));
                        }
                    })
                    .build(k)
                    .key(AUTH_SUBMIT_KEY),
            );
        body = body.child(user_field).child(pass_field).child(footer);

        return Some(modal_frame(p, tab_id, AUTH_CARD_KEY, AUTH_USER_KEY, title, body));
    }

    None
}

fn dismiss_icon_btn(p: &Pal, key: &'static str, label: &'static str, on_press: impl Fn(&mut App) + 'static) -> El {
    el().key(key)
        .role("button")
        .focusable(true)
        .label_static(label)
        .w(24.0)
        .h(24.0)
        .radius(4.0)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .on_press(on_press)
        .child(icon(&ic::CLOSE, 12.0).color(p.icon_muted))
}

/// Official's ghost-muted `IconButton` with its 16 pt `close` icon: `side` 20 (small) or 28 (large).
fn sized_dismiss_btn(
    p: &Pal,
    key: &'static str,
    label: &'static str,
    side: f32,
    radius: f32,
    on_press: impl Fn(&mut App) + 'static,
) -> El {
    el().key(key)
        .role("button")
        .focusable(true)
        .label_static(label)
        .w(side)
        .h(side)
        .radius(radius)
        .center()
        .cursor(Cursor::Pointer)
        .hover(p.hover)
        .on_press(on_press)
        .child(icon(&ic::CLOSE, 16.0).color(p.icon_muted))
}

/// The blocked pop-up's destination row: a web address's host whole, then the rest cut short
/// (`refusal-bar.tsx`'s `hostDisplay`); an isolated tab's or another scheme's address in full
/// (bounded), so the scheme is never hidden.
fn popup_target(p: &Pal, url: &str, isolated: bool) -> Option<El> {
    let lower = url.to_ascii_lowercase();
    let web = !isolated && (lower.starts_with("http://") || lower.starts_with("https://"));
    let row = row().w_full().min_w(0.0).dir(rune::Direction::Ltr);
    if !web {
        let shown = bounded_origin_url(url);
        return (!shown.is_empty()).then(|| {
            row.child(text(shown).font(NOTICE_TARGET).color(p.muted).truncate().grow(1.0).shrink().min_w(0.0))
        });
    }
    let host = short_host(url);
    let after = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = after.find(['/', '?', '#']).map_or("", |i| &after[i..]);
    Some(
        row.child(text(host).font(NOTICE_TARGET).color(p.muted))
            .child(text(rest.to_owned()).font(NOTICE_TARGET).color(p.muted).truncate().shrink().min_w(0.0)),
    )
}

fn modal_frame(
    p: &Pal,
    tab_id: u64,
    key: &'static str,
    refocus_key: &'static str,
    title: String,
    body: El,
) -> El {
    let card = col()
        .key(key)
        .role("alertdialog")
        .focusable(true)
        .label(title)
        .w_full()
        .max_w(480.0) // dialog-size: ok (in-pane browser prompt card matching upstream #63002 prompts.tsx width: min(100%, 480px))
        .padding(16.0)
        .radius(12.0)
        .fill(p.layer1)
        .border(1.0, p.border)
        .elevate(p.overlay)
        .on_key(move |s: &mut App, ev: &KeyEvent, tx: &mut Tx| {
            handle_modal_key(s, tab_id, ev, tx, true)
        })
        .child(body);
    col()
        .key("browser-prompt-backdrop")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .center()
        .padding(16.0)
        .fill(p.scrim)
        // Swallow clicks on the dimmed page and keep keyboard focus inside the dialog (`prompts.tsx:174`).
        .on_press_tx(move |_app: &mut App, tx: &mut Tx| {
            if !super::is_modal_key(tx.focused()) {
                tx.focus(refocus_key, false);
            }
        })
        .child(card)
}

/// Full-origin + bounded path display (max 96 chars) for blocked pop-ups from isolated/preview
/// tabs so the user sees the complete scheme, host, and port before clicking Open.
fn bounded_origin_url(url: &str) -> String {
    let origin = browser_pane::policy::destination_origin(url);
    let shown = origin.as_deref().unwrap_or(url);
    let tail = origin
        .as_deref()
        .and_then(|o| url.get(o.len()..))
        .unwrap_or("");
    let full = if tail.is_empty() || tail == "/" {
        shown.to_owned()
    } else {
        format!("{shown}{tail}")
    };
    if full.chars().count() > 96 {
        let head: String = full.chars().take(95).collect();
        format!("{head}…")
    } else {
        full
    }
}

/// UTF-8 safe left-elided authority display (up to 64 characters, keeping the registrable tail),
/// with bidi overrides and control characters stripped.
fn short_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let clean = browser_pane::prompts::sanitize_single_line_prompt(authority, 256);
    let count = clean.chars().count();
    if count > 64 {
        let tail: String = clean.chars().skip(count - 63).collect();
        format!("…{tail}")
    } else {
        clean
    }
}

fn prompt_site(origin: &str, top_url: &str) -> String {
    let orig = browser_pane::destination_origin(origin);
    let top = browser_pane::destination_origin(top_url);
    let embedded = if orig.is_some() {
        orig != top
    } else {
        // Empty, `about:srcdoc`, `data:`, or `blob:` dialog origins on a web page come from an
        // embedded/sandboxed frame rather than the top-level site (`review-rd-engine.md`).
        top.is_some() || (!origin.is_empty() && origin != top_url)
    };
    let lower = origin.to_ascii_lowercase();
    if lower.starts_with("file:") || lower.starts_with("oc-preview:") {
        return t!("rune.browser.prompt.site.file").to_owned();
    }
    if let Some(o) = orig {
        let host = short_host(&o);
        return if embedded {
            t!("rune.browser.prompt.site.embedded", host = host)
        } else {
            t!("rune.browser.prompt.site.web", host = host)
        };
    }
    if embedded {
        t!("rune.browser.prompt.site.embeddedPage").to_owned()
    } else {
        t!("rune.browser.prompt.site.page").to_owned()
    }
}

fn permission_sentence(origin: &str, kinds: &[PermissionKind]) -> String {
    let is_file = origin.to_ascii_lowercase().starts_with("oc-preview:");
    let host = short_host(origin);
    let has_cam = kinds.contains(&PermissionKind::Camera);
    let has_mic = kinds.contains(&PermissionKind::Microphone);
    if has_cam && has_mic {
        return if is_file {
            t!("rune.browser.prompt.perm.file.cameraMicrophone").to_owned()
        } else {
            t!("rune.browser.prompt.perm.web.cameraMicrophone", host = host)
        };
    }
    match kinds.first().copied().unwrap_or(PermissionKind::Camera) {
        PermissionKind::Camera => if is_file {
            t!("rune.browser.prompt.perm.file.camera").to_owned()
        } else {
            t!("rune.browser.prompt.perm.web.camera", host = host)
        },
        PermissionKind::Microphone => if is_file {
            t!("rune.browser.prompt.perm.file.microphone").to_owned()
        } else {
            t!("rune.browser.prompt.perm.web.microphone", host = host)
        },
        PermissionKind::Location => if is_file {
            t!("rune.browser.prompt.perm.file.location").to_owned()
        } else {
            t!("rune.browser.prompt.perm.web.location", host = host)
        },
        PermissionKind::Notifications => if is_file {
            t!("rune.browser.prompt.perm.file.notifications").to_owned()
        } else {
            t!("rune.browser.prompt.perm.web.notifications", host = host)
        },
        PermissionKind::ClipboardRead => if is_file {
            t!("rune.browser.prompt.perm.file.clipboard").to_owned()
        } else {
            t!("rune.browser.prompt.perm.web.clipboard", host = host)
        },
    }
}
