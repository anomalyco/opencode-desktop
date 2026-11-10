--!strict
-- The Extensions lists (`server-panel.tsx`): the location's MCP servers with their status dots
-- and switches, its plugins and its skills, each read from the server when the popover opens,
-- and the footer under each: the setup row, the configuration file, Manage MCP servers.

local ui = require("@opencode/ui")
local machine = require("./machine")

local lists = {}

-- "mcp", "plugins" or "skills".
export type Service = string

-- The three lists, in the card's order.
local ALL: { Service } = table.freeze({ "mcp", "plugins", "skills" })

function lists.all(): { Service }
    return ALL
end

local PATHS: { [string]: string } = { mcp = "/api/mcp", plugins = "/api/plugin", skills = "/api/skill" }
local ICONS: { [string]: string } = { mcp = "mcp", plugins = "cube", skills = "graduation-cap" }

-- One row of a list: its name, status, error and (a skill's) file.
export type Item = { name: string, status: string?, error: string?, path: string? }

-- A list as read for this open: loading, failed, or its items sorted by name.
export type Load = { kind: "loading" } | { kind: "failed" } | { kind: "ready", items: { Item } }

export type State = {
    -- Bumped on each open of the popover, so each open reads the lists afresh.
    opens: number,
    -- Each list's items, parsed once per answer.
    parsed: { [string]: { generation: number, items: { Item } } },
    -- Each MCP server's switch.
    toggles: { [string]: machine.ToggleState },
    -- Each footer action, by `<service>:<action>`.
    actions: { [string]: machine.ConfigState },
    -- The narrow window's drawer: the card, or one list in its place.
    drawer: Service?,
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
    redraw: () -> (),
}

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
                table.insert(items, { name = name, status = str(status.status), error = str(status.error) })
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
    return { kind = "ready", items = items }, res.generation
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

-- Props every use shares, frozen: the host decodes a frozen table once and copies the node.
local LABELS: { [any]: UiTextProps } = {}
local ICON = table.freeze({ size = 16, color = "icon-weak" } :: UiIconProps)
local ITEM_NAME = table.freeze({ gap = 8, align = "center", grow = 1, shrink = true, minW = 0 } :: UiBoxProps)
local ITEM_END = table.freeze({ ml = 8, gap = 8, align = "center" } :: UiBoxProps)
local DOT_BOX = table.freeze({ w = 16, h = 16, center = true } :: UiBoxProps)
local RULE = table.freeze({ w = "full", mx = -2, my = 2 } :: UiBoxProps)
local RULE_LINE = table.freeze({ w = "full", h = 1, fill = "border-weak" } :: UiBoxProps)

local function label(words: string, color: Color?): ui.Node
    local ink: any = color or "text-chrome"
    local props = LABELS[ink]
    if props == nil then
        props = table.freeze({
            size = FONT.size,
            weight = FONT.weight,
            lineHeight = FONT.lineHeight,
            color = ink,
            truncate = true,
            shrink = true,
            minW = 0,
        } :: UiTextProps)
        LABELS[ink] = props
    end
    return ui.text(words, props)
end

local function inkOf(status: string?): Color
    if status == "connected" or status == "active" then
        return "status-success"
    elseif status == "failed" then
        return "status-danger"
    elseif status == "needs_auth" then
        return "status-warning"
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

-- Steps MCP server `name`'s switch.
local function stepToggle(env: Env, name: string, event: machine.ToggleEvent, run: ((attempt: number) -> ())?)
    local state, effects = machine.toggle(env.state.toggles[name] or machine.toggleIdle, event)
    env.state.toggles[name] = state
    for _, effect in effects do
        if effect.type == "run" and run then
            run(effect.attempt)
        elseif effect.type == "refresh" then
            env.Server:Invalidate("/api/mcp")
        elseif effect.type == "toast-failed" then
            env.Toast:Error(env.I18n:T("requestFailed"), effect.error)
        elseif effect.type == "toast-read-only" then
            env.Toast:Error(env.I18n:T("requestFailed"), env.I18n:T("readOnly"))
        end
    end
    env.redraw()
end

-- Desktop's `follow`: a press asks for the switch's other side, or signs in.
local function toggleMcp(env: Env, item: Item, directory: string)
    local enable = item.status == "needs_auth" or item.status == "disabled"
    stepToggle(env, item.name, { type = "requested", writable = env.Server:CanWrite() }, function(attempt)
        local settled = false
        task.delay(machine.TOGGLE_TIMEOUT, function()
            if not settled then
                stepToggle(env, item.name, { type = "timed-out", attempt = attempt })
            end
        end)
        task.spawn(function()
            local verb = if enable then "connect" else "disconnect"
            local path =
                `/api/experimental/mcp/{env.Server:EncodeUri(item.name)}/{verb}{lists.location(env, directory)}`
            local res = env.Server:RequestAsync("POST", path)
            settled = true
            stepToggle(
                env,
                item.name,
                { type = "settled", attempt = attempt, error = if res.ok then nil else (res.error or "") }
            )
        end)
    end)
end

local function itemRow(
    env: Env,
    view: View,
    service: Service,
    item: Item,
    directory: string,
    minH: number,
    first: boolean
): ui.Node
    local base = {
        key = `details-item-{item.name}`,
        w = "full",
        minH = minH,
        pl = 12,
        py = 4,
        gap = 8,
        align = "center",
        radius = 4,
        mt = if first then nil else 2,
    }
    local disabled = item.status == "disabled"
    local name = label(item.name, if disabled then "text-muted" else nil)
    if service == "mcp" then
        local toggle = env.state.toggles[item.name]
        local busy = toggle ~= nil and toggle.status == "toggling"
        local failure = if item.status == "failed" then item.error else nil
        local props = table.clone(base) :: any
        props.pr = 6
        props.hover = "surface-hover"
        props.hoverTransition = "instant"
        if not busy then
            props.onPress = view:Handler(function()
                if failure then
                    env.Popover:Close()
                    env.Commands:Run("mcp.toggle")
                    return
                end
                toggleMcp(env, item, directory)
            end)
        end
        return ui.row(props, {
            ui.row(ITEM_NAME, { dot(item.status, failure), name }),
            ui.row(ITEM_END, {
                ui.box(
                    { opacity = if busy then 0.5 else 1 },
                    { ui.switch({ key = `details-switch-{item.name}`, checked = not disabled }) }
                ),
            }),
        })
    elseif service == "plugins" then
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
    props.onPress = view:Handler(function()
        env.Popover:Close()
        if item.path then
            env.Links:Open({ href = item.path })
        end
    end)
    return ui.row(props, { ui.icon("file-document", ICON), name })
end

-- Under the list: a rule, then (when empty, or for skills) the setup row, the configuration file,
-- and MCP's Manage MCP servers.
local function footer(env: Env, view: View, service: Service, empty: boolean, directory: string, minH: number): ui.Node
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
    return ui.col({ key = `details-footer-{service}`, w = "full", pt = 6, px = 2 }, rows)
end

-- A list's rows, then its footer: the flyout beside the card, or the narrow window's drawer.
function lists.pane(env: Env, view: View, service: Service, directory: string, mobile: boolean): ui.Node
    local load = lists.read(env, service, directory)
    local minH = if mobile then 44 else 28
    local body: { ui.Node } = {}
    if load.kind == "loading" then
        body = { message(env.I18n:T("loading")) }
    elseif load.kind == "failed" then
        body = {
            message(env.I18n:T("requestFailed")),
            configRow(
                "details-retry",
                nil,
                env.I18n:T("retry"),
                minH,
                view:Handler(function()
                    env.Server:Invalidate("/api/")
                end)
            ),
        }
    elseif #load.items == 0 then
        body = {
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
    else
        for i, item in load.items do
            table.insert(body, itemRow(env, view, service, item, directory, minH, i == 1))
        end
    end
    local children: { ui.Node } = {
        ui.col({ w = "full", px = 2, shrink = true, minH = 0, scrollY = true }, body),
    }
    if load.kind == "ready" then
        table.insert(children, footer(env, view, service, #load.items == 0, directory, minH))
    end
    return ui.col({
        key = `details-pane-{service}`,
        role = "group",
        label = env.I18n:T(service),
        w = "full",
        maxH = if mobile then nil else 480,
    }, children)
end

return lists
