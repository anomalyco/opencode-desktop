--!strict
-- Site information under the address field's site button (panel.tsx `SiteInformation`,
-- site-machine.ts): what the page can do and what can reach it: its connection, its cookies
-- (counted, and cleared), the permissions it never gets, the agent's access, and the network it
-- loads through. Each read and clear takes the next attempt number, so an answer whose attempt
-- the state no longer holds is stale.

local ui = require("@opencode/ui")
local address = require("./address")

local site = {}

-- A read that hasn't answered by then fails, so the popover offers a retry.
site.LOAD_TIMEOUT = 5
-- A clear and the page's reload after it; past this the clear counts as failed.
site.CLEAR_TIMEOUT = 35

export type State =
    { status: "closed", attempt: number }
    | { status: "loading", attempt: number }
    | { status: "ready", attempt: number, cookies: number }
    | { status: "failed", attempt: number }
    | { status: "clearing", attempt: number }

export type Event =
    { type: "opened" }
    | { type: "retry" }
    | { type: "load.succeeded", attempt: number, cookies: number }
    | { type: "load.failed", attempt: number }
    | { type: "clear.clicked" }
    | { type: "clear.succeeded", attempt: number }
    | { type: "clear.failed", attempt: number }
    | { type: "timeout", phase: "loading" | "clearing", attempt: number }

export type Effect = { type: "load", attempt: number } | { type: "clear", attempt: number } | { type: "notify-clear-failed" }

local INITIAL: State = { status = "closed", attempt = 0 }
site.initial = INITIAL

local function load(attempt: number): (State, { Effect })
    return { status = "loading", attempt = attempt }, { { type = "load", attempt = attempt } }
end

-- A clear that settled reads the count again; a failure also tells the user.
local function cleared(state: State, attempt: number, effects: { Effect }): (State, { Effect })
    if state.status ~= "clearing" or attempt ~= state.attempt then
        return state, {}
    end
    local nextState, more = load(state.attempt + 1)
    local all = table.clone(effects)
    for _, e in more do
        table.insert(all, e)
    end
    return nextState, all
end

function site.step(state: State, event: Event): (State, { Effect })
    if event.type == "opened" then
        if state.status == "loading" or state.status == "clearing" then
            return state, {}
        end
        return load(state.attempt + 1)
    elseif event.type == "retry" then
        if state.status == "failed" then
            return load(state.attempt + 1)
        end
        return state, {}
    elseif event.type == "load.succeeded" then
        if state.status ~= "loading" or event.attempt ~= state.attempt then
            return state, {}
        end
        return { status = "ready", attempt = state.attempt, cookies = event.cookies }, {}
    elseif event.type == "load.failed" then
        if state.status ~= "loading" or event.attempt ~= state.attempt then
            return state, {}
        end
        return { status = "failed", attempt = state.attempt }, {}
    elseif event.type == "clear.clicked" then
        if state.status ~= "ready" then
            return state, {}
        end
        return { status = "clearing", attempt = state.attempt + 1 }, { { type = "clear", attempt = state.attempt + 1 } }
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
        return { status = "failed", attempt = state.attempt }, {}
    end
end

-- How a page reached its content: HTTPS, HTTP on this machine's loopback, HTTP elsewhere, or a
-- workspace file.
local function connection(url: string): string?
    local scheme, host = address.parts(url)
    local s = string.lower(scheme)
    if s == "https://" then
        return "secure"
    elseif s == "file://" then
        return "file"
    elseif s == "http://" then
        local name = string.match(host, "^%[[^%]]*%]") or string.match(host, "^[^:]*") or host
        return if address.loopback(name) then "local" else "insecure"
    end
    return nil
end

local CONNECTIONS: { [string]: { icon: string, title: string, description: string? } } = {
    secure = { icon = "lock", title = "site.secure" },
    insecure = { icon = "outline-hexagonal-warning", title = "site.insecure", description = "site.insecure.description" },
    ["local"] = { icon = "monitor", title = "site.local", description = "site.local.description" },
    file = { icon = "folder", title = "site.file", description = "site.file.description" },
}

local BODY = table.freeze({ size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-chrome" })
local DETAIL = table.freeze({ size = 11, weight = "regular", lineHeight = 16, tracking = 0.05, color = "text-muted", wrap = true })

-- `browser-site-row`: the icon centred on the first line of the text, 8 between, 4 by 8 in.
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

export type Env = {
    I18n: I18nService,
    -- The plugin's site state, and a step of it.
    state: () -> State,
    dispatch: (event: Event) -> (),
    -- The server the window talks to, when it's another machine's.
    server: () -> string?,
}

-- The popover's card, for page `url`.
function site.render(env: Env, view: View, url: string): ui.Node
    local state = env.state()
    local _, host = address.parts(url)
    local items: { ui.Node } = {
        ui.row({ w = "full", px = 12, py = 4, gap = 8, align = "start" }, {
            ui.row({ h = 20, align = "center" }, { ui.icon("outline-globe", { size = 16, color = "icon-weak" }) }),
            ui.text(if host ~= "" then host else url, { dir = "ltr", size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-chrome", truncate = true, noWrap = true, grow = 1, shrink = true, minW = 0 }),
        }),
        -- Edge to edge, a 1 px line scaled to a hairline (half its colour).
        ui.col({ w = "full", h = 1, my = 2, fill = "border-base", opacity = 0.5 }),
    }
    local kind = connection(url)
    local c = if kind then CONNECTIONS[kind] else nil
    if c then
        table.insert(items, row(c.icon, env.I18n:T(c.title), if c.description then env.I18n:T(c.description) else nil, nil))
    end
    -- Only web pages have cookies and ask for permissions; a workspace file has neither.
    if kind and kind ~= "file" then
        local detail: string? = nil
        if state.status == "failed" then
            detail = env.I18n:T("common.requestFailed")
        elseif state.status == "ready" then
            detail = env.I18n:T(if state.cookies == 1 then "site.cookies.count.one" else "site.cookies.count.other", { count = state.cookies })
        end
        local failed = state.status == "failed"
        local enabled = state.status == "ready" or failed
        local button = ui.button({
            key = "browser-site-clear",
            text = env.I18n:T(if failed then "common.retry" else "site.cookies.clear"),
            size = "small",
            variant = "neutral",
            disabled = not enabled,
            onPress = view:Handler(function()
                env.dispatch(if failed then { type = "retry" } else { type = "clear.clicked" })
            end),
        })
        table.insert(items, row("outline-cookie", env.I18n:T("site.cookies"), detail, button))
        table.insert(items, row("outline-eye-slash", env.I18n:T("site.permissions"), env.I18n:T("site.permissions.description"), nil))
    end
    table.insert(items, row("select-element", env.I18n:T("site.agent"), env.I18n:T("site.agent.description"), nil))
    local server = env.server()
    if server then
        table.insert(items, row("server", env.I18n:T("site.network", { server = server }), nil, nil))
    end
    return ui.col({ key = "browser-site", role = "region", label = env.I18n:T("site.info"), w = 261, py = 4, gap = 2, radius = 8, fill = "surface-1", shadow = "floating" }, items)
end

return site
