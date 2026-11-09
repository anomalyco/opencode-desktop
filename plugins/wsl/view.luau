--!strict
-- Add WSL server, drawn (Desktop's `dialog.tsx`, `dialog.css`): the installed distros to pick
-- from with each one's status, Check again, Need another distro? opening the catalog of distros
-- to install, and the setup card when WSL itself isn't there, each in the host's dialog frame (480,
-- as tall as its content). Desktop's `.settings-wsl-dialog` and `.settings-wsl-not-installed-dialog`
-- rules never match (the class lands on the dialog's content, not its root), so neither changes
-- the frame or the header and footer padding.

local ui = require("@opencode/ui")

local view = {}

-- `text-13-regular` with Desktop's -0.04 tracking.
local BODY = { size = 13, weight = "regular", lineHeight = 16, tracking = -0.04, color = "text-chrome" }

local function t(env: any, text: any): string
    return env.I18n:T(text.key, text.params)
end

-- WSL isn't installed (the error names it, or there's none), rather than broken.
local function missing(err: string?): boolean
    if not err then
        return true
    end
    local e = string.lower(err)
    return string.find(e, "wsl is not installed", 1, true) ~= nil
        or string.find(e, "not been installed", 1, true) ~= nil
        or string.find(e, "wsl --install", 1, true) ~= nil
        or string.find(e, "wsl.exe --install", 1, true) ~= nil
end

-- What the host's frame holds.
local function container(children: { UiNode }): UiNode
    return ui.col({ key = "wsl-dialog", w = "full" }, children)
end

local function loading(): UiNode
    return container({ ui.row({ w = "full", px = 24, py = 48, center = true }, { ui.loader({ size = 16 }) }) })
end

-- `[data-slot="dialog-footer"]`: its buttons at its end, 8 apart, 16 in.
local function footer(buttons: { UiNode }): UiNode
    local children = { ui.spacer() }
    for _, b in buttons do
        table.insert(children, b)
    end
    return ui.row({ w = "full", gap = 8, pad = 16 }, children)
end

local function radioRow(v: View, key: string, label: string, description: string, tone: string?, checked: boolean, disabled: boolean, rule: boolean, minH: number, pick: () -> ()): UiNode
    local r = ui.row({
        key = key,
        w = "full",
        minH = minH,
        pt = 14,
        pr = 14,
        pb = 14,
        pl = 16,
        role = "radio",
        label = label,
        onPress = if disabled then nil else v:Handler(pick),
    }, {
        ui.radio({ text = label, description = description, checked = checked, disabled = disabled, tone = tone :: any, lineHeight = 16, grow = 1 }),
    })
    if rule then
        return ui.col({ w = "full" }, { r, ui.box({ w = "full", h = 0.5, fill = "border-base" }) })
    end
    return r
end

local function header(env: any, title: string): UiNode
    return ui.col({ w = "full" }, {
        ui.row({ w = "full", pad = 16, align = "center" }, { ui.text(title, { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text-chrome" }) }),
        ui.box({ w = "full", h = 0.5, fill = "border-strong" }),
    })
end

-- "Need another distro?": opens the catalog.
local function catalogCard(env: any, v: View, m: any): UiNode
    return ui.row({
        key = "wsl-catalog-card",
        role = "button",
        label = env.I18n:T("onboarding.needAnotherDistro"),
        w = "full",
        minH = 64,
        pt = 14,
        pr = 14,
        pb = 16,
        pl = 16,
        gap = 12,
        radius = 8,
        border = "border-base",
        borderWidth = 0.5,
        opacity = if m.busy then 0.5 else 1,
        onPress = if m.busy then nil else v:Handler(function()
            env.controller.dispatch({ type = "catalog.opened", defaultTarget = if m.installable[1] then m.installable[1].name else nil, busy = m.busy })
        end),
    }, {
        ui.icon("download", { size = 16, color = "icon-base" }),
        ui.col({ grow = 1, shrink = true, minW = 0, gap = 6 }, {
            ui.text(env.I18n:T("onboarding.needAnotherDistro"), BODY),
            -- 12 tall in Desktop's CSS, but its button lays the line out 14 tall.
            ui.text(env.I18n:T("onboarding.needAnotherDistroHint"), { size = 12, weight = "regular", lineHeight = 14, tracking = 0.05, color = "text-muted" }),
        }),
        ui.icon("chevron", { size = 16, color = "icon-weak" }),
    })
end

local function mainView(env: any, v: View, m: any): { UiNode }
    local c = env.controller
    local checkAgain = ui.button({
        key = "wsl-check-again",
        text = env.I18n:T("onboarding.checkAgain"),
        variant = "ghost-muted",
        size = "small",
        disabled = m.busy,
        onPress = v:Handler(function()
            c.dispatch({ type = "distros.refresh.requested", busy = c.model().busy })
        end),
    })
    local section = ui.row({ w = "full", h = 24, pl = 9, gap = 12, align = "center", justify = "between" }, {
        ui.text(env.I18n:T("onboarding.installedDistros"), BODY),
        checkAgain,
    })
    local rows: { UiNode } = {}
    if #m.addable == 0 then
        local words = if #m.visibleInstalled > 0 then env.I18n:T("onboarding.allDistrosAdded") else env.I18n:T("onboarding.noDistros")
        table.insert(rows, ui.text(words, { size = 13, weight = "regular", lineHeight = 18.2, color = "text-muted", pad = 16 }))
    else
        for i, d in m.addable do
            local status = m.statuses[d.name]
            local disabled = d.version == 1 or m.busy
            local tone = if status and status.tone ~= "muted" then status.tone else nil
            local name = d.name
            table.insert(rows, radioRow(v, `wsl-distro-{name}`, name, if status then t(env, status.label) else "", tone, m.selected == name, disabled, i < #m.addable, 62, function()
                c.dispatch({ type = "distro.selected", distro = name })
            end))
        end
    end
    local list = ui.col({ key = "wsl-distro-list", w = "full", maxH = 220, radius = 8, fill = "bg-base", shadow = "raised", scrollY = true, clip = true }, rows)
    local bodyChildren = { section, list }
    if #m.installable > 0 then
        table.insert(bodyChildren, catalogCard(env, v, m))
    end
    local pb = m.primary
    local primary = ui.button({
        key = "wsl-primary",
        text = t(env, pb.label),
        variant = if pb.variant == "contrast" then "contrast" else "neutral",
        loading = pb.loading,
        disabled = not pb.loading and pb.disabled,
        w = pb.width,
        onPress = v:Handler(function()
            local now = c.model()
            local button = now.primary
            c.dispatch({ type = "primary.requested", distro = now.selected, action = if button.action == "install-opencode" then "install-opencode" elseif button.action == "add" then "add" else nil, disabled = button.disabled or button.loading })
        end),
    })
    local cancel = ui.button({
        key = "wsl-cancel",
        text = env.I18n:T("common.cancel"),
        variant = "neutral",
        disabled = m.adding,
        onPress = v:Handler(function()
            env.close()
        end),
    })
    return {
        header(env, env.I18n:T("server.add")),
        ui.col({ w = "full", pt = 16, px = 16, pb = 1, gap = 16 }, bodyChildren),
        footer({ cancel, primary }),
    }
end

local function catalogView(env: any, v: View, m: any): { UiNode }
    local c = env.controller
    local field = ui.field({ key = "wsl-catalog-search", editor = env.search, placeholder = env.I18n:T("onboarding.searchDistros"), size = "large", disabled = m.busy, w = "full" })
    local rows: { UiNode } = {}
    for i, d in m.filtered do
        local name = d.name
        table.insert(rows, radioRow(v, `wsl-catalog-{name}`, d.label, "", nil, m.catalogTarget == name, m.busy, i < #m.filtered, 48, function()
            c.dispatch({ type = "catalog.target.changed", distro = name })
        end))
    end
    local list = ui.col({ key = "wsl-catalog-list", w = "full", maxH = 220, radius = 8, border = "border-strong", borderWidth = 0.5, scrollY = true, clip = true }, rows)
    local cancel = ui.button({
        key = "wsl-catalog-cancel",
        text = env.I18n:T("common.cancel"),
        variant = "neutral",
        disabled = m.busy,
        onPress = v:Handler(function()
            c.dispatch({ type = "catalog.closed", busy = c.model().busy })
        end),
    })
    local installing = m.installingCatalog
    local install = ui.button({
        key = "wsl-catalog-install",
        text = env.I18n:T("onboarding.installDistro"),
        variant = if installing then "neutral" else "contrast",
        loading = installing,
        disabled = not installing and (m.busy or m.catalogTarget == nil),
        w = 99,
        onPress = v:Handler(function()
            local now = c.model()
            c.dispatch({ type = "catalog.install.requested", distro = now.catalogTarget, busy = now.busy })
        end),
    })
    return {
        header(env, env.I18n:T("onboarding.installDistro")),
        ui.col({ w = "full", pt = 16, px = 16, pb = 1, gap = 12 }, { field, list }),
        footer({ cancel, install }),
    }
end

-- `DialogWslSetup`: WSL not installed, unavailable, or waiting on a restart.
local function setupCard(env: any, v: View, state: string, err: string?, installable: boolean, busy: boolean): UiNode
    local title, description
    if state == "pendingRestart" then
        title, description = env.I18n:T("onboarding.restartRequired"), env.I18n:T("onboarding.windowsRestartRequired")
    elseif installable then
        title, description = env.I18n:T("onboarding.wslNotInstalled.title"), env.I18n:T("onboarding.wslNotInstalled.description")
    else
        title, description = env.I18n:T("onboarding.wslUnavailable.title"), env.I18n:T("onboarding.wslUnavailable.description")
    end
    local message: { UiNode } = {
        -- dialog.tsx fills it #DBDBDB in either theme.
        ui.icon("warning", { size = 24, color = ui.hex("#DBDBDB") }),
        ui.text(title, { size = 15, weight = "medium", lineHeight = 20, tracking = -0.13, tabular = true, color = "text-chrome" }),
        ui.text(description, { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, tabular = true, maxW = 300, wrap = true, color = "text-muted", textAlign = "center" }),
    }
    if not installable and err then
        table.insert(message, ui.text(err, { size = 12, weight = "regular", lineHeight = 18, tracking = -0.04, maxW = 360, wrap = true, mt = 4, color = "text-faint", textAlign = "center" }))
    end
    local content: { UiNode } = { ui.col({ w = "full", gap = 8, align = "center" }, message) }
    if state == "unavailable" and installable then
        table.insert(content, ui.button({
            key = "wsl-install",
            text = env.I18n:T("onboarding.installWsl"),
            variant = "neutral",
            disabled = busy,
            onPress = v:Handler(function()
                env.controller.dispatch({ type = "wsl.install.requested", busy = env.controller.model().busy })
            end),
        }))
    end
    if state ~= "unavailable" then
        table.insert(content, ui.button({
            key = "wsl-close",
            text = env.I18n:T("common.close"),
            variant = "neutral",
            onPress = v:Handler(function()
                env.close()
            end),
        }))
    end
    return container({ ui.col({ key = "wsl-setup", w = "full", py = 56, gap = 24, align = "center" }, content) })
end

function view.dialog(env: any, v: View): UiNode?
    local c = env.controller
    local m = c.model()
    if m.runtimeState == "checking" or m.runtimeState == "loading" then
        return loading()
    end
    if m.runtimeState ~= "ready" then
        local api = env.api()
        local err = if api and api.state.runtime then api.state.runtime.error else env.I18n:T("error.unavailable")
        return setupCard(env, v, m.runtimeState, err, missing(err), m.busy)
    end
    return container(if m.view == "main" then mainView(env, v, m) else catalogView(env, v, m))
end

return view
