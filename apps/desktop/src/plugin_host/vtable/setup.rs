//! What a setup registers: commands, panels, settings rows, and every other contribution kind.
//! A window half registers its UI; an app half only its app menu items.

use std::ffi::c_void;

use opencode_plugin::abi::*;

use super::super::icons::intern;
use super::super::instance::{CallbackEnv, CommandRec, ContribRec, PanelRec, QueryEnv, RenderEnv, Scope, SettingRec};
use super::{Kind, SetupCtx, cast, text};

/// The setup context `ctx` is, when it's a window half's.
///
/// # Safety
/// As `cast`.
unsafe fn window<'a>(ctx: *mut c_void) -> Option<&'a mut SetupCtx<'a>> {
    // SAFETY: as `cast`.
    unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }.filter(|s| s.scope == Scope::Window)
}

pub(super) unsafe extern "C" fn add_command(ctx: *mut c_void, spec: *const CommandSpecAbi) -> AbiStatus {
    // SAFETY: the plugin passes its setup context and a spec valid for the call.
    let (Some(s), Some(spec)) = (unsafe { window(ctx) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    if text(spec.id).is_empty() || spec.run.func.is_none() {
        return AbiStatus::Err;
    }
    let bind = if cfg!(target_os = "windows") {
        spec.bind_windows
    } else if cfg!(target_os = "macos") {
        spec.bind_macos
    } else {
        spec.bind_linux
    };
    let keybind = text(bind).parse::<rune::Keybind>().ok().filter(|k| !k.chords().is_empty());
    let slash = text(spec.slash).trim_start_matches('/');
    s.made.commands.push(CommandRec {
        id: intern(&format!("{}.{}", s.plugin_id, text(spec.id))),
        title: intern(text(spec.title)),
        description: intern(text(spec.description)),
        slash: (!slash.is_empty()).then(|| intern(slash)),
        keybind,
        editable: spec.flags & COMMAND_EDITABLE != 0,
        hidden: spec.flags & COMMAND_HIDDEN != 0,
        run: CallbackEnv::new(spec.run, s.lib.clone()),
    });
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn add_panel(ctx: *mut c_void, spec: *const PanelSpecAbi) -> AbiStatus {
    // SAFETY: as `add_command`.
    let (Some(s), Some(spec)) = (unsafe { window(ctx) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    let id = text(spec.id).to_owned();
    if id.is_empty() || id.contains(':') || spec.render.func.is_none() {
        return AbiStatus::Err;
    }
    s.made.panels.push(PanelRec {
        tab: intern(&format!("{}.{id}", s.plugin_id)),
        id,
        title: text(spec.title).to_owned(),
        description: text(spec.description).to_owned(),
        icon: text(spec.icon).to_owned(),
        order: spec.order,
        flags: spec.flags,
        render: RenderEnv::new(spec.render, s.lib.clone()),
        label: spec.label.func.is_some().then(|| RenderEnv::new(spec.label, s.lib.clone())),
    });
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn add_setting(ctx: *mut c_void, spec: *const SettingSpecAbi) -> AbiStatus {
    // SAFETY: as `add_command`.
    let (Some(s), Some(spec)) = (unsafe { window(ctx) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    if spec.id.is_empty() || spec.render.func.is_none() {
        return AbiStatus::Err;
    }
    s.made.settings.push(SettingRec {
        id: text(spec.id).to_owned(),
        title: text(spec.title).to_owned(),
        description: text(spec.description).to_owned(),
        keywords: text(spec.keywords).to_owned(),
        render: RenderEnv::new(spec.render, s.lib.clone()),
    });
    AbiStatus::Ok
}

pub(super) unsafe extern "C" fn add_contribution(ctx: *mut c_void, spec: *const ContributionAbi) -> AbiStatus {
    // SAFETY: the plugin passes its setup context (either half) and a spec valid for the call.
    let (Some(s), Some(spec)) = (unsafe { cast::<SetupCtx>(ctx, Kind::Setup) }, unsafe { spec.as_ref() }) else { return AbiStatus::Err };
    let id = text(spec.id).to_owned();
    // What each kind needs; an app half contributes only to the app menu.
    let needs = match spec.kind {
        contrib::MENU_ITEM | contrib::MENUBAR_ITEM | contrib::CODE_BLOCK_ACTION => spec.run.func.is_some(),
        contrib::SETTINGS_PAGE | contrib::SLOT | contrib::FILE_VIEWER | contrib::CODE_BLOCK => spec.render.func.is_some(),
        contrib::TITLEBAR_ITEM | contrib::COMPOSER_PILL | contrib::POPOVER => spec.render.func.is_some(),
        contrib::LINK_HANDLER => spec.run.func.is_some() && spec.query.func.is_some(),
        contrib::MCP_TOOL => spec.query.func.is_some(),
        contrib::THEME | contrib::SERVER => true,
        _ => false,
    };
    let scoped = s.scope == Scope::Window || spec.kind == contrib::MENUBAR_ITEM;
    if id.is_empty() || id.contains(':') || !needs || !scoped {
        return AbiStatus::Err;
    }
    if spec.kind == contrib::THEME && !super::super::contrib::register_theme(s.plugin_id, &id, text(spec.title), text(spec.target)) {
        return AbiStatus::Err;
    }
    let lib = s.lib;
    s.made.contribs.push(ContribRec {
        kind: spec.kind,
        scope: s.scope,
        key: intern(&format!("{}.{id}", s.plugin_id)),
        id,
        title: text(spec.title).to_owned(),
        description: text(spec.description).to_owned(),
        keywords: text(spec.keywords).to_owned(),
        icon: text(spec.icon).to_owned(),
        target: text(spec.target).to_owned(),
        order: spec.order,
        flags: spec.flags,
        run: spec.run.func.is_some().then(|| CallbackEnv::new(spec.run, lib.clone())),
        render: spec.render.func.is_some().then(|| RenderEnv::new(spec.render, lib.clone())),
        query: spec.query.func.is_some().then(|| QueryEnv::new(spec.query, lib.clone())),
    });
    AbiStatus::Ok
}
