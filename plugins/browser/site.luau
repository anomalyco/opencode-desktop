--!strict
-- Site information under the address field's site button matching upstream
-- `packages/gui-plugins/src/browser/panel.tsx` `SiteInformation` (`live-browser-site-info-dark.png`)
-- and `site-machine.ts`: two views in one popover — status-first connection info + "Site settings"
-- entry, then detailed cookies/cache/permissions/agent/network controls with Back navigation.

local ui = require("@opencode/ui")
local address = require("./address")
local hostDisplay = require("./host_display")

local site = {}

-- A read that hasn't answered by then fails, so the popover offers a retry.
site.LOAD_TIMEOUT = 5
-- A clear and the page's reload after it; past this the clear counts as failed.
site.CLEAR_TIMEOUT = 35

export type State =
    { status: "closed", attempt: number, detailed: boolean }
    | { status: "loading", attempt: number, detailed: boolean }
    | { status: "ready", attempt: number, cookies: number, detailed: boolean }
    | { status: "failed", attempt: number, detailed: boolean }
    | { status: "clearing", attempt: number, detailed: boolean }

export type Event =
    { type: "opened" }
    | { type: "closed" }
    | { type: "details", shown: boolean }
    | { type: "retry" }
    | { type: "load.succeeded", attempt: number, cookies: number }
    | { type: "load.failed", attempt: number }
    | { type: "clear.clicked" }
    | { type: "clear-cache.clicked" }
    | { type: "clear.succeeded", attempt: number }
    | { type: "clear.failed", attempt: number }
    | { type: "timeout", phase: "loading" | "clearing", attempt: number }

export type Effect =
    { type: "load", attempt: number }
    | { type: "clear", attempt: number }
    | { type: "clear-cache", attempt: number }
    | { type: "notify-clear-failed" }

local INITIAL: State = { status = "closed", attempt = 0, detailed = false }
site.initial = INITIAL

local function load(attempt: number, detailed: boolean): (State, { Effect })
    return { status = "loading", attempt = attempt, detailed = detailed }, { { type = "load", attempt = attempt } }
end

local function withDetails(state: State, detailed: boolean): State
    if state.status == "ready" then
        return { status = "ready", attempt = state.attempt, cookies = state.cookies, detailed = detailed }
    end
    return { status = state.status, attempt = state.attempt, detailed = detailed } :: State
end

-- A clear that settled reads the count again; a failure also tells the user.
local function cleared(state: State, attempt: number, effects: { Effect }): (State, { Effect })
    if state.status ~= "clearing" or attempt ~= state.attempt then
        return state, {}
    end
    local nextState, more = load(state.attempt + 1, state.detailed)
    local all = table.clone(effects)
    for _, e in more do
        table.insert(all, e)
    end
    return nextState, all
end

function site.step(state: State, event: Event): (State, { Effect })
    if event.type == "opened" then
        if state.status == "loading" or state.status == "clearing" then
            return withDetails(state, false), {}
        end
        return load(state.attempt + 1, false)
    elseif event.type == "closed" then
        return withDetails(state, false), {}
    elseif event.type == "details" then
        return withDetails(state, event.shown), {}
    elseif event.type == "retry" then
        if state.status == "failed" then
            return load(state.attempt + 1, state.detailed)
        end
        return state, {}
    elseif event.type == "load.succeeded" then
        if state.status ~= "loading" or event.attempt ~= state.attempt then
            return state, {}
        end
        return { status = "ready", attempt = state.attempt, cookies = event.cookies, detailed = state.detailed }, {}
    elseif event.type == "load.failed" then
        if state.status ~= "loading" or event.attempt ~= state.attempt then
            return state, {}
        end
        return { status = "failed", attempt = state.attempt, detailed = state.detailed }, {}
    elseif event.type == "clear.clicked" then
        if state.status ~= "ready" then
            return state, {}
        end
        return { status = "clearing", attempt = state.attempt + 1, detailed = state.detailed }, { { type = "clear", attempt = state.attempt + 1 } }
    elseif event.type == "clear-cache.clicked" then
        if state.status ~= "ready" then
            return state, {}
        end
        return { status = "clearing", attempt = state.attempt + 1, detailed = state.detailed }, { { type = "clear-cache", attempt = state.attempt + 1 } }
    elseif event.type == "clear.succeeded" then
        return cleared(state, event.attempt, {})
    elseif event.type == "clear.failed" then
        return cleared(state, event.attempt, { { type = "notify-clear-failed" } })
    else
        if event.phase == "clearing" then
            return cleared(state, event.attempt, { { type = "notify-clear-failed" } })
        end
        if state.status ~= "loading" or event.attempt ~= state.attempt then
            return state, {}
        end
        return { status = "failed", attempt = state.attempt, detailed = state.detailed }, {}
    end
end

-- How a page reached its content: HTTPS, HTTP on loopback, HTTP elsewhere, workspace file, or preview.
local function connection(url: string): string?
    local scheme, host = address.parts(url)
    local s = string.lower(scheme)
    if s == "https://" then
        return "secure"
    elseif s == "file://" then
        return "file"
    elseif s == "workspace://" or s == "oc-preview://" then
        return "workspace"
    elseif s == "http://" then
        local name = string.match(host, "^%[[^%]]*%]") or string.match(host, "^[^:]*") or host
        return if address.loopback(name) then "local" else "insecure"
    end
    return nil
end

local CONNECTIONS: { [string]: { icon: string, title: string, description: string } } = {
    secure = { icon = "lock", title = "site.secure", description = "site.secure.description" },
    insecure = { icon = "outline-hexagonal-warning", title = "site.insecure", description = "site.insecure.description" },
    ["local"] = { icon = "monitor", title = "site.local", description = "site.local.description" },
    file = { icon = "folder", title = "site.file", description = "site.file.description" },
    workspace = { icon = "folder", title = "preview.workspace", description = "site.file.description" },
}

local BODY = table.freeze({ size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-chrome" })
local DETAIL = table.freeze({ size = 11, weight = "regular", lineHeight = 16, tracking = 0.05, color = "text-muted", wrap = true })

local function row(icon: string, title: string, detail: string?, action: ui.Node?): ui.Node
    local lines: { ui.Node } = { ui.text(title, BODY :: any) }
    if detail then
        table.insert(lines, ui.text(detail, DETAIL :: any))
    end
    local items: { ui.Node } = {
        ui.row({ h = 20, align = "center" }, { ui.icon(icon :: any, { size = 16, color = "icon-weak" }) }),
        ui.col({ grow = 1, shrink = true, minW = 0, gap = 2 }, lines),
    }
    if action then
        table.insert(items, ui.col({ h = "full", justify = "center" }, { action }))
    end
    return ui.row({ w = "full", px = 12, py = 4, gap = 8, align = "start" }, items)
end

local function separator(): ui.Node
    return ui.col({ w = "full", h = 1, my = 2, fill = "border-base", opacity = 0.5 })
end

export type Env = {
    I18n: I18nService,
    state: () -> State,
    dispatch: (event: Event) -> (),
    server: () -> string?,
    canClearCache: () -> boolean,
}

-- Renders the two-view Site information popover (`live-browser-site-info-dark.png`).
function site.render(env: Env, view: View, url: string): ui.Node
    local state = env.state()
    local shownHost = hostDisplay.displayHost(url)
    if shownHost == "" then
        local _, rawHost = address.parts(url)
        shownHost = if rawHost ~= "" then rawHost else url
    end
    local kind = connection(url)
    local isWeb = kind ~= nil and kind ~= "file" and kind ~= "workspace"

    -- View 1: status-first connection overview + "Site settings >" drill-in.
    if not state.detailed then
        local items: { ui.Node } = {
            ui.row({ key = "browser-site-host", w = "full", px = 12, py = 4, gap = 8, align = "start" }, {
                ui.row({ h = 20, align = "center" }, { ui.icon("outline-globe", { size = 16, color = "icon-weak" }) }),
                ui.text(shownHost, { dir = "ltr", size = 13, weight = "medium", lineHeight = 20, tracking = -0.04, color = "text-chrome", truncate = true, noWrap = true, grow = 1, shrink = true, minW = 0 }),
            }),
            separator(),
        }
        local c = if kind then CONNECTIONS[kind] else nil
        if c then
            table.insert(items, row(c.icon, env.I18n:T(c.title), env.I18n:T(c.description), nil))
        end
        table.insert(items, ui.row({
            key = "browser-site-settings",
            role = "button",
            label = env.I18n:T("site.settings"),
            w = "full",
            px = 12,
            py = 6,
            gap = 8,
            radius = 6,
            align = "center",
            hover = "surface-hover",
            press = "surface-press",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                env.dispatch({ type = "details", shown = true })
            end),
        }, {
            ui.icon("outline-sliders", { size = 16, color = "icon-weak" }),
            ui.text(env.I18n:T("site.settings"), BODY :: any, { grow = 1, shrink = true, minW = 0 }),
            ui.icon("chevron-right", { size = 14, color = "icon-weak" }),
        }))
        return ui.col({
            key = "browser-site-card",
            w = 312,
            py = 8,
            gap = 4,
            radius = 8,
            fill = "surface-1",
            border = "border-base",
            borderWidth = 0.5,
            shadow = "floating",
        }, items)
    end

    -- View 2: "Site settings" — Cookies, Saved files (when engine supports cache clear), Permissions, Agent, Network.
    local items: { ui.Node } = {
        ui.row({ key = "browser-site-details-header", w = "full", px = 8, py = 4, gap = 8, align = "center" }, {
            ui.iconButton({
                key = "browser-site-back",
                icon = "outline-arrow-left",
                variant = "ghost-muted",
                size = "small",
                label = env.I18n:T("site.back"),
                onPress = view:Handler(function()
                    env.dispatch({ type = "details", shown = false })
                end),
            }),
            ui.text(env.I18n:T("site.settings"), BODY :: any, { weight = "medium" }),
            ui.text(shownHost, { dir = "ltr", size = 11, weight = "regular", lineHeight = 16, color = "text-muted", truncate = true, noWrap = true, grow = 1, shrink = true, minW = 0, textAlign = "end" }),
        }),
        separator(),
    }
    if isWeb then
        local detail: string? = nil
        if state.status == "failed" then
            detail = env.I18n:T("site.cookies.failed")
        elseif state.status == "ready" then
            detail = env.I18n:T(if state.cookies == 1 then "site.cookies.count.one" else "site.cookies.count.other", { count = state.cookies })
        end
        local failed = state.status == "failed"
        local enabled = state.status == "ready" or failed
        local clearCookiesBtn = ui.button({
            key = "browser-site-clear",
            text = env.I18n:T(if failed then "common.retry" else "site.cookies.clear"),
            size = "small",
            variant = "neutral",
            disabled = not enabled,
            onPress = if enabled then view:Handler(function()
                env.dispatch({ type = if failed then "retry" else "clear.clicked" })
            end) else nil,
        })
        table.insert(items, row("outline-cookie", env.I18n:T("site.cookies"), detail, clearCookiesBtn))

        if env.canClearCache() then
            local cacheEnabled = state.status == "ready"
            local clearCacheBtn = ui.button({
                key = "browser-site-clear-cache",
                text = env.I18n:T("site.cookies.clear"),
                label = env.I18n:T("site.clearCache"),
                size = "small",
                variant = "neutral",
                disabled = not cacheEnabled,
                onPress = if cacheEnabled then view:Handler(function()
                    env.dispatch({ type = "clear-cache.clicked" })
                end) else nil,
            })
            table.insert(items, row("file-document", env.I18n:T("site.cache"), env.I18n:T("site.clearCache.description"), clearCacheBtn))
        end

        table.insert(items, row("outline-eye-slash", env.I18n:T("site.permissions"), env.I18n:T("site.permissions.description"), nil))
    end
    table.insert(items, row("select-element", env.I18n:T("site.agent"), env.I18n:T("site.agent.description"), nil))
    local remote = env.server()
    if remote and remote ~= "" then
        table.insert(items, row("server", env.I18n:T("site.network", { server = remote }), nil, nil))
    end
    return ui.col({
        key = "browser-site-card",
        w = 312,
        py = 8,
        gap = 4,
        radius = 8,
        fill = "surface-1",
        border = "border-base",
        borderWidth = 0.5,
        shadow = "floating",
    }, items)
end

return site
