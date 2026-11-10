--!strict
-- The Extensions lists (`server-panel.tsx`): the location's MCP servers with their status dots
-- and switches, its plugins and its skills, each read from the server when the popover opens,
-- and the footer under each: the setup row, the configuration file, Manage MCP servers.

local ui = require("@opencode/ui")
local machine = require("./machine")
local mcp = require("./mcp")

local lists = {}

local Theme: ThemeService = nil :: any

-- The theme service, for the status dots.
function lists.init(theme: ThemeService)
    Theme = theme
end

-- "mcp", "plugins" or "skills".
export type Service = string

-- The three lists, in the card's order.
local ALL: { Service } = table.freeze({ "mcp", "plugins", "skills" })

function lists.all(): { Service }
    return ALL
end

local PATHS: { [string]: string } = { mcp = "/api/mcp", plugins = "/api/plugin", skills = "/api/skill" }
local ICONS: { [string]: string } = { mcp = "mcp", plugins = "cube", skills = "graduation-cap" }

-- One row of a list: its name, status, error, (a skill's) file and (an MCP server's) sign-in.
export type Item = { name: string, status: string?, error: string?, path: string?, integrationID: string? }

-- A list as read for this open: loading, failed, or its items sorted by name.
export type Load = { kind: "loading" } | { kind: "failed" } | { kind: "ready", items: { Item } }

export type State = {
    -- Bumped on each open of the popover, so each open reads the lists afresh.
    opens: number,
    -- Each list's items, parsed once per answer.
    parsed: { [string]: { generation: number, items: { Item } } },
    -- Each footer action, by `<service>:<action>`.
    actions: { [string]: machine.ConfigState },
}

export type Env = {
    Server: ServerService,
    Servers: ServersService,
    I18n: I18nService,
    Popover: PopoverService,
    Layout: LayoutService,
    Links: LinksService,
    Commands: CommandsService,
    System: SystemService,
    Toast: ToastService,
    state: State,
    -- The MCP servers' toggles, sign-ins and error dialog.
    mcp: mcp.Env,
    redraw: () -> (),
}

-- Where a list shows: beside the card (`session-service-pane`), or in the narrow window's More
-- drawer (`session-service-drawer`), `compact` below 640 points, where an MCP row is 44 tall.
export type Placement = { kind: "flyout" } | { kind: "drawer", compact: boolean }

-- `?location[directory]=…`, or nothing for the server's default location.
function lists.location(env: Env, directory: string): string
    if directory == "" then
        return ""
    end
    return `?location%5Bdirectory%5D={env.Server:EncodeUri(directory)}`
end

local function str(v: any): string?
    return if type(v) == "string" and v ~= "" then v else nil
end

local function entries(json: any): { any }
    if type(json) ~= "table" then
        return {}
    end
    return if type(json.data) == "table" then json.data else json
end

local function parse(service: Service, json: any): { Item }
    local items: { Item } = {}
    for _, m in entries(json) do
        if type(m) ~= "table" then
            continue
        end
        if service == "mcp" then
            local name = str(m.name)
            local status = if type(m.status) == "table" then m.status else {}
            if name then
                table.insert(
                    items,
                    { name = name, status = str(status.status), error = str(status.error), integrationID = str(m.integrationID) }
                )
            end
        elseif service == "plugins" then
            local source = if type(m.source) == "table" then m.source else {}
            if source.type ~= "builtin" then
                local name = str(m.id) or str(source.target) or str(source.path) or str(source.type)
                local state = if type(m.state) == "table" then m.state else {}
                local status = str(state.status)
                if name then
                    table.insert(
                        items,
                        { name = name, status = status, error = if status == "failed" then str(state.error) else nil }
                    )
                end
            end
        else
            local name = str(m.name)
            if name then
                table.insert(items, { name = name, path = str(m.path) })
            end
        end
    end
    table.sort(items, function(a, b)
        return string.lower(a.name) < string.lower(b.name)
    end)
    return items
end

local synced: (env: Env, directory: string, items: { Item }) -> ()

-- `service`'s list for directory `directory`, as read for this open.
function lists.read(env: Env, service: Service, directory: string): (Load, number)
    local path = PATHS[service] .. lists.location(env, directory)
    local res = env.Server:Resource(path, env.state.opens)
    if res.status == "error" then
        return { kind = "failed" }, res.generation
    end
    if res.status == "loading" and res.generation == 0 then
        return { kind = "loading" }, 0
    end
    local cached = env.state.parsed[path]
    if cached and cached.generation == res.generation then
        return { kind = "ready", items = cached.items }, res.generation
    end
    local items = parse(service, res.json)
    env.state.parsed[path] = { generation = res.generation, items = items }
    if service == "mcp" then
        synced(env, directory, items)
    end
    return { kind = "ready", items = items }, res.generation
end

-- The error dialog follows its server's status, which changes here and anywhere else: a new
-- answer of the list reaches the machine (`server.synced`) once the render is done.
synced = function(env: Env, directory: string, items: { Item })
    local dialog = mcp.state(env.mcp, directory).dialog
    if dialog.status ~= "open" then
        return
    end
    local attempt, status = dialog.attempt, nil
    for _, item in items do
        if item.name == dialog.name then
            status = { status = item.status or "pending", error = item.error }
        end
    end
    task.defer(function()
        mcp.dispatch(env.mcp, directory, { type = "server.synced", attempt = attempt, loaded = true, status = status })
    end)
end

-- `/api/mcp`'s servers, for the MCP controller.
function lists.servers(json: any): { Item }
    return parse("mcp", json)
end

function lists.icon(service: Service): string
    return ICONS[service]
end

-- Whether any item of a ready list failed, and how many items it has.
function lists.summary(load: Load): (boolean, number)
    if load.kind ~= "ready" then
        return false, 0
    end
    for _, item in load.items do
        if item.status == "failed" then
            return true, #load.items
        end
    end
    return false, #load.items
end

local FONT = { size = 13, weight = "regular" :: FontWeight, lineHeight = 16 }

-- A row's words; `name` (a server's, a plugin's, a skill's) reads in its own direction, as
-- `TruncatedText` does.
-- Props every use shares, frozen: the host decodes a frozen table once and copies the node.
local LABELS: { [string]: UiTextProps } = {}
local ICON = table.freeze({ size = 16, color = "icon-weak" } :: UiIconProps)
local ITEM_NAME = table.freeze({ gap = 8, align = "center", grow = 1, shrink = true, minW = 0 } :: UiBoxProps)
local ITEM_END = table.freeze({ ml = 8, gap = 8, align = "center" } :: UiBoxProps)
local DOT_BOX = table.freeze({ w = 16, h = 16, center = true } :: UiBoxProps)
local RULE = table.freeze({ w = "full", mx = -2, my = 2 } :: UiBoxProps)
local RULE_LINE = table.freeze({ w = "full", h = 1, fill = "border-weak" } :: UiBoxProps)

local function label(words: string, color: Color?, name: boolean?): ui.Node
    local ink: any = color or "text-chrome"
    local key = `{if type(ink) == "string" then ink else tostring(ink)}:{name == true}`
    local props = LABELS[key]
    if props == nil then
        props = table.freeze({
            size = FONT.size,
            weight = FONT.weight,
            lineHeight = FONT.lineHeight,
            color = ink,
            truncate = true,
            shrink = true,
            minW = 0,
            dir = if name then "auto" else nil,
        } :: UiTextProps)
        LABELS[key] = props
    end
    return ui.text(words, props)
end

-- `.session-service-dot`'s colour: Desktop's v1 `--icon-success-base`, `--icon-critical-base` and
-- `--icon-warning-base` on the default theme (the app's `widgets::status_dot`), a chosen theme's
-- state colours, else `icon-faint`.
local function inkOf(status: string?): Color
    local v1 = Theme:Current().id == "oc-2"
    local dark = Theme:IsDark()
    if status == "connected" or status == "active" then
        return if v1 then ui.hex(if dark then "#089b00" else "#0abe00") else "status-success"
    elseif status == "failed" then
        return if v1 then ui.hex(if dark then "#cb321c" else "#fa3012") else "status-danger"
    elseif status == "needs_auth" then
        return if v1 then ui.hex(if dark then "#fbb73c" else "#ebb76e") else "status-warning"
    end
    return "icon-weaker"
end

-- A 6 dot: filled, or a hollow ring for a disabled server, its error on hover.
local function dot(status: string?, err: string?): ui.Node
    local ink = inkOf(status) :: any
    if status == "disabled" then
        return ui.box({ w = 6, h = 6, radius = 3, border = ink, borderWidth = 1, help = err })
    end
    return ui.box({ w = 6, h = 6, radius = 3, fill = ink, help = err })
end

-- `session-service-config`: an icon and a label, as a menu row.
local function configRow(
    key: string,
    glyph: string?,
    words: string,
    minH: number,
    onPress: ui.Handler?,
    disabled: boolean?
): ui.Node
    local children: { ui.Node } = {}
    if glyph then
        table.insert(children, ui.icon(glyph, ICON))
    end
    table.insert(children, label(words))
    return ui.row({
        key = key,
        role = "button",
        label = words,
        w = "full",
        minH = minH,
        px = 12,
        py = 4,
        gap = 8,
        align = "center",
        radius = 4,
        hover = if disabled then nil else "surface-hover",
        hoverTransition = "instant",
        onPress = if disabled then nil else onPress,
    }, children)
end

local function message(words: string): ui.Node
    return ui.text(
        words,
        { size = FONT.size, weight = FONT.weight, lineHeight = FONT.lineHeight, color = "text-faint", px = 12, py = 6 }
    )
end

-- Steps footer action `key` with `event` and runs what it asks for.
local function stepAction(env: Env, key: string, event: machine.ConfigEvent, run: ((attempt: number) -> ())?)
    local state, effects = machine.config(env.state.actions[key] or machine.configIdle, event)
    env.state.actions[key] = state
    for _, effect in effects do
        if effect.type == "run" and run then
            run(effect.attempt)
        elseif effect.type == "schedule-reset" then
            local attempt = effect.attempt
            task.delay(machine.COPIED_FOR, function()
                stepAction(env, key, { type = "copied.expired", attempt = attempt })
            end)
        elseif effect.type == "close-owner" then
            env.Popover:Close()
        elseif effect.type == "toast-failed" then
            env.Toast:Error(env.I18n:T("requestFailed"), effect.error)
        elseif effect.type == "toast-timed-out" then
            env.Toast:Error(env.I18n:T("requestFailed"), env.I18n:T("timedOut"))
        end
    end
    env.redraw()
end

-- `serviceConfigFile`: the last document listing any of the service's entries, else the last
-- setting it, else the first.
local function configFile(list: { any }, service: Service): string?
    local docs: { any } = {}
    for _, e in list do
        local path = if type(e) == "table" then e.path else nil
        if
            e.type == "document"
            and type(path) == "string"
            and (path:sub(-5) == ".json" or path:sub(-6) == ".jsonc")
        then
            table.insert(docs, e)
        end
    end
    local function names(d: any): boolean
        local info = if type(d.info) == "table" then d.info else {}
        if service == "mcp" then
            local servers = if type(info.mcp) == "table" then info.mcp.servers else nil
            return type(servers) == "table" and next(servers) ~= nil
        end
        local v = info[service]
        return type(v) == "table" and #v > 0
    end
    local function sets(d: any): boolean
        return type(d.info) == "table" and d.info[service] ~= nil
    end
    for i = #docs, 1, -1 do
        if names(docs[i]) then
            return docs[i].path
        end
    end
    for i = #docs, 1, -1 do
        if sets(docs[i]) then
            return docs[i].path
        end
    end
    return if docs[1] then docs[1].path else nil
end

-- `globalConfigDirectory`: the first config root unless it's a project's `.opencode`, else the
-- location itself.
local function globalDirectory(list: { any }, directory: string): string
    for _, e in list do
        if type(e) == "table" and e.type == "directory" and type(e.path) == "string" then
            local root = e.path:gsub("[/\\]+$", "")
            if root:sub(-9) == ".opencode" then
                return directory
            end
            return root
        end
    end
    return directory
end

-- Reads the location's configuration, then opens (or copies) the file holding `service`, or with
-- `setup` starts a draft in the global configuration's folder that sets it up.
local function configAction(env: Env, service: Service, directory: string, setup: boolean)
    local key = `{service}:{if setup then "setup" else "config"}`
    stepAction(env, key, { type = "activate" }, function(attempt)
        local settled = false
        task.delay(machine.CONFIG_TIMEOUT, function()
            if not settled then
                stepAction(env, key, { type = "timed-out", attempt = attempt })
            end
        end)
        task.spawn(function()
            local res = env.Server:RequestAsync("GET", "/api/config" .. lists.location(env, directory))
            settled = true
            if not res.ok then
                return stepAction(env, key, { type = "failed", attempt = attempt, error = res.error or "" })
            end
            local list = entries(res.json)
            if setup then
                env.Layout:NewDraft(globalDirectory(list, directory), env.I18n:T(`setup.{service}.prompt`))
                return stepAction(env, key, { type = "opened", attempt = attempt, close = true })
            end
            local path = configFile(list, service)
            if not path then
                return stepAction(
                    env,
                    key,
                    { type = "failed", attempt = attempt, error = env.I18n:T("configFileMissing") }
                )
            end
            -- A file on this machine opens in a side tab beside the session; a remote one's path is copied.
            if env.Server:IsLocal() then
                env.Links:Open({ href = path })
                return stepAction(env, key, { type = "opened", attempt = attempt, close = true })
            end
            env.System:CopyText(path)
            stepAction(env, key, { type = "copied", attempt = attempt })
        end)
    end)
end

-- `StatusDot` of an MCP row (`.session-mcp-name .session-service-dot`): the 6 dot alone. A failed
-- one shows its error on hover, and a press on it, 16 around, opens the full error.
local function mcpDot(env: Env, view: View, item: Item, directory: string): ui.Node
    local failure = if item.status == "failed" then item.error else nil
    if failure == nil then
        return dot(item.status, nil)
    end
    return ui.box({
        key = `details-dot-{item.name}`,
        role = "button",
        label = env.I18n:T("mcp.error.actionLabel", { name = item.name }),
        help = failure,
        helpSide = "top",
        w = 16,
        h = 16,
        m = -5,
        center = true,
        onPress = view:Handler(function()
            mcp.dispatch(env.mcp, directory, { type = "error.opened", name = item.name, error = failure :: string })
        end),
    }, { dot(item.status, nil) })
end

-- `.session-mcp-sign-in`: a quiet small ghost-muted button in the switch's place, 20 tall, 5 in
-- and 5 out at the end, 11 on 16 at 440, tracking 0.05; Signing in while its toggle runs.
local function signInButton(env: Env, item: Item, busy: boolean, pending: boolean, onPress: ui.Handler): ui.Node
    local words = env.I18n:T(if busy then "mcp.signingIn" else "mcp.signIn")
    return ui.row({
        key = `details-sign-in-{item.name}`,
        role = "button",
        label = env.I18n:T("mcp.signIn.open", { name = item.name }),
        h = 20,
        px = 5,
        mr = -5,
        center = true,
        radius = 4,
        opacity = if pending then 0.5 else nil,
        hover = if pending then nil else "surface-hover",
        press = if pending then nil else "surface-press",
        hoverTransition = "instant",
        onPress = if pending then nil else onPress,
    }, {
        ui.text(words, { size = 11, weight = "regular", lineHeight = 16, tracking = 0.05, color = "text-muted", noWrap = true }),
    })
end

-- `session-mcp-row`: its status dot and name, then its switch, or Sign in for a server that needs
-- one. A press follows the row: a failed server opens its error, any other turns over.
local function mcpRow(env: Env, view: View, item: Item, directory: string, base: any, compact: boolean): ui.Node
    local state = mcp.state(env.mcp, directory)
    local busy, pending = machine.mcpRow(state, item.name, item.status)
    local disabled = item.status == "disabled"
    local failure = if item.status == "failed" then item.error else nil
    local function change(enabled: boolean)
        mcp.dispatch(env.mcp, directory, { type = "toggle.requested", name = item.name, enabled = enabled, status = item.status })
    end
    local follow = view:Handler(function()
        if failure then
            mcp.dispatch(env.mcp, directory, { type = "error.opened", name = item.name, error = failure })
            return
        end
        change(item.status == "needs_auth" or disabled)
    end)
    local props = table.clone(base)
    props.pr = 6
    -- Below 640 points every MCP row stands 44 tall, 8 in above and below.
    if compact then
        props.minH = 44
        props.py = 8
    end
    props.hover = if pending then nil else "surface-hover"
    props.hoverTransition = "instant"
    props.onPress = follow
    local trailing = if item.status == "needs_auth" and item.integrationID
        then signInButton(env, item, busy, pending, follow)
        -- `ui.switch` keeps a settings row's 4 around it; here the 16 switch sits 6 from the
        -- row's top, bottom and end (`.session-mcp-row`).
        else ui.box({ m = -4, opacity = if pending then 0.5 else nil }, {
            ui.switch({
                key = `details-switch-{item.name}`,
                checked = not disabled,
                disabled = pending,
                onChange = view:Handler(function(event)
                    change(event.checked == true)
                end),
            }),
        })
    return ui.row(props, {
        ui.row(ITEM_NAME, {
            mcpDot(env, view, item, directory),
            label(item.name, if disabled then "text-muted" else nil, true),
        }),
        ui.row(ITEM_END, { trailing }),
    })
end

local function itemRow(
    env: Env,
    view: View,
    service: Service,
    item: Item,
    directory: string,
    placement: Placement,
    first: boolean
): ui.Node
    local base = {
        key = `details-item-{item.name}`,
        w = "full",
        minH = 28,
        pl = 12,
        py = 4,
        gap = 8,
        align = "center",
        radius = 4,
        mt = if first then nil else 2,
    }
    if service == "mcp" then
        return mcpRow(env, view, item, directory, base, placement.kind == "drawer" and placement.compact)
    end
    local name = label(item.name, if item.status == "disabled" then "text-muted" else nil, true)
    if service == "plugins" then
        local props = table.clone(base) :: any
        props.pr = 12
        local children: { ui.Node } = {}
        if item.status then
            table.insert(children, ui.box(DOT_BOX, { dot(item.status, item.error) }))
        end
        table.insert(children, name)
        return ui.row(props, children)
    end
    local props = table.clone(base) :: any
    props.pr = 12
    props.role = "button"
    props.hover = "surface-hover"
    props.hoverTransition = "instant"
    -- A skill opens its SKILL.md in a side tab; the popover closes as it does.
    props.onPress = view:Handler(function()
        if item.path then
            env.Links:Open({ href = item.path })
        end
        if placement.kind == "flyout" then
            env.Popover:Close()
        end
    end)
    return ui.row(props, { ui.icon("file-document", ICON), name })
end

-- Under the list (`session-service-footer`): a rule, then (when empty, or for skills) the setup
-- row, the configuration file, and MCP's Manage MCP servers; 6 under the list, and 2 in from
-- the flyout's edges as the list is (the drawer's card already insets it).
local function footer(env: Env, view: View, service: Service, empty: boolean, directory: string, placement: Placement): ui.Node
    local minH = 28
    local rows: { ui.Node } = {
        ui.col(RULE, { ui.box(RULE_LINE) }),
    }
    if empty or service == "skills" then
        local key = `{service}:setup`
        local state = env.state.actions[key]
        table.insert(
            rows,
            configRow(
                `details-setup-{service}`,
                "plus",
                env.I18n:T(`setup.{service}`),
                minH,
                view:Handler(function()
                    configAction(env, service, directory, true)
                end),
                state ~= nil and state.status == "opening"
            )
        )
    end
    local isLocal = env.Server:IsLocal()
    local config = env.state.actions[`{service}:config`]
    local copied = config ~= nil and config.status == "copied"
    local glyph = if isLocal then "settings-gear" elseif copied then "check" else "outline-copy"
    local words = env.I18n:T(if isLocal then "configure" else "copyConfigPath")
    local row = configRow(
        `details-config-{service}`,
        glyph,
        words,
        minH,
        view:Handler(function()
            configAction(env, service, directory, false)
        end),
        config ~= nil and config.status == "opening"
    )
    table.insert(
        rows,
        if isLocal
            then row
            else ui.box(
                { w = "full", help = env.I18n:T(if copied then "copied" else "copy"), helpSide = "top" },
                { row }
            )
    )
    if service == "mcp" and not empty then
        table.insert(
            rows,
            configRow(
                "details-manage-mcp",
                "mcp",
                env.I18n:T("mcp.manage"),
                minH,
                view:Handler(function()
                    env.Popover:Close()
                    env.Commands:Run("mcp.toggle")
                end)
            )
        )
    end
    local inset = if placement.kind == "flyout" then 2 else nil
    return ui.col({ key = `details-footer-{service}`, w = "full", pt = 6, px = inset }, rows)
end

-- A list's body (`ServiceBody`): Loading, Request failed with Retry, the empty state, or its rows.
local function body(env: Env, view: View, service: Service, load: Load, directory: string, placement: Placement): { ui.Node }
    if load.kind == "loading" then
        return { message(env.I18n:T("loading")) }
    elseif load.kind == "failed" then
        -- `session-service-message` holds the words and the Retry row (`session-summary-row`, 36
        -- clear at its end on the drawer's card).
        return {
            ui.col({ key = "details-failed", role = "alert", w = "full", px = 12, py = 6 }, {
                ui.text(env.I18n:T("requestFailed"), {
                    size = FONT.size,
                    weight = FONT.weight,
                    lineHeight = FONT.lineHeight,
                    color = "text-faint",
                    wrap = true,
                }),
                ui.row({
                    key = "details-retry",
                    role = "button",
                    label = env.I18n:T("retry"),
                    w = "full",
                    minH = 28,
                    pl = 12,
                    pr = if placement.kind == "drawer" then 36 else 12,
                    py = 4,
                    align = "center",
                    radius = 4,
                    hover = "surface-hover",
                    hoverTransition = "instant",
                    onPress = view:Handler(function()
                        env.Server:Invalidate("/api/")
                    end),
                }, { label(env.I18n:T("retry")) }),
            }),
        }
    elseif #load.items == 0 then
        return {
            ui.col({ w = "full" }, {
                ui.text(env.I18n:T(`{service}.empty`), {
                    size = FONT.size,
                    weight = FONT.weight,
                    lineHeight = FONT.lineHeight,
                    color = "text-faint",
                    px = 12,
                    py = 8,
                }),
            }),
        }
    end
    local rows: { ui.Node } = {}
    for i, item in load.items do
        table.insert(rows, itemRow(env, view, service, item, directory, placement, i == 1))
    end
    return rows
end

-- A list's rows, then its footer, beside the card (`session-service-pane`: the rows scroll in
-- 480 at most, 2 in from the flyout's edges).
function lists.pane(env: Env, view: View, service: Service, directory: string): ui.Node
    local placement: Placement = { kind = "flyout" }
    local load = lists.read(env, service, directory)
    local children: { ui.Node } = {
        ui.col({ w = "full", px = 2, shrink = true, minH = 0, scrollY = true }, body(env, view, service, load, directory, placement)),
    }
    if load.kind == "ready" then
        table.insert(children, footer(env, view, service, #load.items == 0, directory, placement))
    end
    return ui.col({
        key = `details-pane-{service}`,
        role = "group",
        label = env.I18n:T(service),
        w = "full",
        maxH = 480,
    }, children)
end

-- A list as the narrow window's More drawer shows it in its own view (`session-summary-card
-- session-service-drawer`): one card, its rows and footer straight on it, 2 between rows and 2
-- more above each row after the first.
function lists.drawer(env: Env, view: View, service: Service, directory: string, compact: boolean, dark: boolean): ui.Node
    local placement: Placement = { kind = "drawer", compact = compact }
    local load = lists.read(env, service, directory)
    local children = body(env, view, service, load, directory, placement)
    if load.kind == "ready" then
        table.insert(children, footer(env, view, service, #load.items == 0, directory, placement))
    end
    return ui.col({
        key = `details-drawer-{service}`,
        role = "group",
        label = env.I18n:T(service),
        w = "full",
        pad = 2,
        gap = 2,
        radius = 6,
        fill = if dark then "surface-1" else "bg-base",
        shadow = "raised",
    }, children)
end

return lists
