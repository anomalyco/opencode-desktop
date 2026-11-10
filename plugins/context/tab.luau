--!strict
-- The Context tab (tab.tsx): the session's figures in a two-column grid (one column under 32rem),
-- its system prompt, and its raw messages, each opening to its JSON, with Export session. The
-- messages are the server's own (`GET /api/session/:id/message`), read again whenever the
-- session changes.

local ui = require("@opencode/ui")
local json = require("./json")

local tab = {}

-- Export session: idle, or reading and saving one session's messages.
export type Export = { kind: "idle" } | { kind: "exporting", sessionId: string }

-- One raw message: its text as the server wrote it and what the tab reads off it.
type Raw = { text: string, value: any, id: string, kind: string }

export type State = {
    -- Raw message rows shown open, by message id.
    open: { [string]: boolean },
    -- Moves each time a row opens or shuts, for the tab's memo version.
    toggles: number,
    export: Export,
    -- Each session's messages as last read, split once per answer.
    parsed: { [string]: { generation: number, raws: { Raw } } },
}

export type Env = {
    Sessions: SessionsService,
    Models: ModelsService,
    Server: ServerService,
    I18n: I18nService,
    Theme: ThemeService,
    Toast: ToastService,
    Workspace: WorkspaceService,
    state: State,
}

local function messagesPath(session: string): string
    return `/api/session/{session}/message?limit=200&order=asc`
end

-- The last step with tokens, after the last finished compaction (`lastAssistantWithUsage`).
local function lastWithUsage(raws: { Raw }): Raw?
    local after = 1
    for i = #raws, 1, -1 do
        local v = raws[i].value
        if v.type == "compaction" and v.status == "completed" then
            after = i + 1
            break
        end
    end
    for i = #raws, after, -1 do
        local v = raws[i].value
        if v.type == "assistant" and v.tokens ~= nil then
            return raws[i]
        end
    end
    return nil
end

-- Props every use shares, frozen: the host decodes a frozen table once and copies the node.
local STAT = table.freeze({ gap = 4, w = 0, grow = 1, minW = 0 } :: UiBoxProps)
local STAT_NAME = table.freeze({ size = 13, weight = "regular" :: FontWeight, lineHeight = 19, color = "text-weak" } :: UiTextProps)
-- Two 19.5 lines on whole pixels: a row is 59 tall, as Desktop's.
local STAT_VALUE = table.freeze({
    size = 13,
    weight = "medium" :: FontWeight,
    lineHeight = 20,
    color = "text-strong",
    selectable = true,
} :: UiTextProps)
local STAT_LINE = table.freeze({ w = "full", gap = 16 } :: UiBoxProps)
local STATS = table.freeze({ key = "context-stats", w = "full", gap = 16 } :: UiBoxProps)
local PERCENT = table.freeze({ style = "percent" } :: NumberFormat)
local CURRENCY = table.freeze({ style = "currency" } :: NumberFormat)
local HEADING = table.freeze({ size = 13, weight = "regular" :: FontWeight, lineHeight = 19.5, color = "text-weak" } :: UiTextProps)
local SYSTEM = table.freeze({ key = "context-system", w = "full", gap = 8 } :: UiBoxProps)
local SYSTEM_BOX =
    table.freeze({ w = "full", px = 12, py = 8, radius = 6, border = "border-base", fill = "surface-1" } :: UiBoxProps)
local SYSTEM_TEXT = table.freeze({
    size = 13,
    weight = "regular" :: FontWeight,
    lineHeight = 19.5,
    wrap = true,
    selectable = true,
    color = "text-chrome",
} :: UiTextProps)
local CONTENT = table.freeze({ key = "session-usage-content", w = "full", pl = 24, pr = 32, pt = 16, pb = 40, gap = 40 } :: UiBoxProps)
local TAB = table.freeze({ key = "context-tab", w = "full", h = "full", pt = 8 } :: UiBoxProps)
local TAB_MOBILE = table.freeze({ key = "context-tab", w = "full", h = "full", pt = 0 } :: UiBoxProps)
local SCROLL = table.freeze({ key = "context-scroll", w = "full", grow = 1, minH = 0, scrollY = true } :: UiBoxProps)
local EXPORT = table.freeze({
    key = "context-export",
    role = "button",
    h = 24,
    px = 8,
    gap = 6,
    align = "center",
    radius = 4,
    hover = "surface-hover",
    press = "surface-press",
    hoverTransition = "instant",
    hoverGroup = true,
} :: UiBoxProps)
local EXPORT_ICON = table.freeze({ size = 14, color = "text-weak", hoverGroupColor = "text-chrome" } :: UiIconProps)
local EXPORT_TEXT = table.freeze({
    size = 13,
    weight = "medium" :: FontWeight,
    lineHeight = 20,
    tracking = -0.04,
    tabular = true,
    color = "text-weak",
    hoverGroupColor = "text-chrome",
} :: UiTextProps)
local RAW_SECTION = table.freeze({ key = "context-raw-section", w = "full", gap = 8 } :: UiBoxProps)
local RAW_HEAD = table.freeze({ w = "full", align = "center", justify = "between" } :: UiBoxProps)
-- A message's row: its kind and id at the start (the id cut to fit), its time and the chevron
-- at the end. One row of four: the id grows into the space between, 16 short of the time, as
-- the kind and id's group, a spacer and the time's group laid out 8 apart.
local RAW_ROW = table.freeze({
    role = "button",
    w = "full",
    h = 32,
    px = 12,
    gap = 8,
    align = "center",
    border = "border-base",
    fill = "bg-base",
    hover = "surface-hover",
    hoverTransition = "instant",
} :: UiBoxProps)
-- By whether the row starts the list, then whether it ends it shut: its corners.
local RAW_CORNERS = table.freeze({
    [0] = table.freeze({ [0] = table.freeze({ 0, 0, 0, 0 }), [1] = table.freeze({ 0, 0, 8, 8 }) }),
    [1] = table.freeze({ [0] = table.freeze({ 8, 8, 0, 0 }), [1] = table.freeze({ 8, 8, 8, 8 }) }),
})
local RAW_BODY = table.freeze({
    w = "full",
    pad = 12,
    corners = { 0, 0, 0, 0 },
    border = "border-base",
    mt = -1,
    fill = "bg-base",
} :: UiBoxProps)
local RAW_BODY_LAST = table.freeze({
    w = "full",
    pad = 12,
    corners = { 0, 0, 8, 8 },
    border = "border-base",
    mt = -1,
    fill = "bg-base",
} :: UiBoxProps)
local RAW_WRAP_FIRST = table.freeze({ w = "full" } :: UiBoxProps)
local RAW_WRAP = table.freeze({ w = "full" } :: UiBoxProps)
local RAW_KIND = table.freeze({
    size = 13,
    weight = "regular" :: FontWeight,
    lineHeight = 19.5,
    color = "text-chrome",
    noWrap = true,
} :: UiTextProps)
local RAW_ID = table.freeze({
    size = 13,
    weight = "regular" :: FontWeight,
    lineHeight = 19.5,
    color = "text-base",
    truncate = true,
    shrink = true,
    minW = 0,
    grow = 1,
    ml = -8,
} :: UiTextProps)
local RAW_TIME = table.freeze({
    size = 13,
    weight = "regular" :: FontWeight,
    lineHeight = 19.5,
    color = "text-weak",
    noWrap = true,
    ml = 8,
} :: UiTextProps)
local RAW_CHEVRON = table.freeze({ size = 14, color = "text-weak", ml = 4 } :: UiIconProps)
local RAW_JSON = table.freeze({
    size = 12,
    weight = "regular" :: FontWeight,
    lineHeight = 18,
    mono = true,
    wrap = true,
    selectable = true,
    color = "text-chrome",
} :: UiTextProps)

local function stats(env: Env, session: Session?, sid: string, raws: { Raw }, wide: boolean): ui.Node
    local I18n = env.I18n
    local last = lastWithUsage(raws)
    -- The step's tokens and model: off the server's messages, or, before they're read (a run
    -- without a server), as the app holds the session.
    local tokens: { input: number, output: number, reasoning: number, read: number, write: number }? = nil
    local provider: string?, model: string? = nil, nil
    if last then
        local t = last.value.tokens
        local n = function(v: any): number
            return if type(v) == "number" then v else 0
        end
        local cache = t.cache or {}
        tokens = {
            input = n(t.input),
            output = n(t.output),
            reasoning = n(t.reasoning),
            read = n(cache.read),
            write = n(cache.write),
        }
        local m = last.value.model or {}
        provider, model = tostring(m.providerID or ""), tostring(m.id or "")
    elseif #raws == 0 and session and session.usage then
        local u = session.usage
        tokens =
            { input = u.input, output = u.output, reasoning = u.reasoning, read = u.cacheRead, write = u.cacheWrite }
        provider, model = session.usageProviderId, session.usageModelId
    end
    local catalog = if provider and model then env.Models:Get(provider, model) else nil
    local total = if tokens then tokens.input + tokens.output + tokens.reasoning + tokens.read + tokens.write else nil
    local limit = if catalog then catalog.contextLimit else nil
    local usage = if total and limit and limit > 0 then math.round(total / limit * 100) else nil
    local function num(v: number?): string
        return if v then I18n:Number(v) else "-"
    end
    local function count(kind: string): number
        local n = 0
        for _, m in raws do
            if m.value.type == kind then
                n += 1
            end
        end
        return n
    end
    local function time(ms: number?): string
        return if ms and ms > 0 then I18n:Date(ms, "dateTime") else "-"
    end
    local providerName = if provider and catalog then catalog.providerName or provider else "-"
    local modelName = if model and catalog then catalog.modelName or model else "-"
    local title = if session then session.title elseif sid == "" then "-" else sid
    local messages = if #raws == 0 then (if session then session.messages or 0 else 0) else #raws
    local function stat(name: string, value: string): ui.Node
        return ui.col(STAT, { ui.text(I18n:T(name), STAT_NAME), ui.text(value, STAT_VALUE) })
    end
    local cells: { ui.Node } = {
        stat("stats.session", title),
        stat("stats.messages", I18n:Number(messages)),
        stat("stats.provider", providerName),
        stat("stats.model", modelName),
        stat("stats.limit", num(limit)),
        stat("stats.totalTokens", num(total)),
        stat("stats.usage", if usage then I18n:Number(usage / 100, PERCENT) else "-"),
        stat("stats.inputTokens", num(tokens and tokens.input)),
        stat("stats.outputTokens", num(tokens and tokens.output)),
        stat("stats.reasoningTokens", num(tokens and tokens.reasoning)),
        ui.col(STAT, {
            ui.text(I18n:T("stats.cacheTokens"), STAT_NAME),
            ui.text("{1} / {2}", STAT_VALUE, num(tokens and tokens.read), num(tokens and tokens.write)),
        }),
        stat("stats.userMessages", I18n:Number(count("user"))),
        stat("stats.assistantMessages", I18n:Number(count("assistant"))),
        stat("stats.totalCost", I18n:Number(if session then session.cost else 0, CURRENCY)),
        stat("stats.sessionCreated", time(session and session.created)),
        stat("stats.lastActivity", time(last and last.value.time and last.value.time.created)),
    }
    local lines: { ui.Node } = table.create(if wide then 8 else 16)
    if wide then
        for i = 1, #cells, 2 do
            table.insert(lines, ui.row(STAT_LINE, { cells[i], cells[i + 1] }))
        end
    else
        for _, cell in cells do
            table.insert(lines, ui.row(STAT_LINE, { cell }))
        end
    end
    return ui.col(STATS, lines)
end

-- The latest system message's text, trimmed; nil when there is none.
local function promptOf(raws: { Raw }): string?
    for i = #raws, 1, -1 do
        if raws[i].value.type == "system" then
            local text = raws[i].value.text
            local prompt = if type(text) == "string" then text:match("^%s*(.-)%s*$") else nil
            return if prompt == "" then nil else prompt
        end
    end
    return nil
end

local function systemPrompt(env: Env, prompt: string): ui.Node
    return ui.col(SYSTEM, {
        ui.text(env.I18n:T("systemPrompt.title"), HEADING),
        ui.col(SYSTEM_BOX, { ui.text(prompt, SYSTEM_TEXT) }),
    })
end

-- `sessionExportFilename`: the title (or slug, or id) lowercased, runs of anything but letters,
-- digits, `_` and `-` as one `-`, at most 250, trimmed of `-`, plus `.json`.
local function filename(info: any, id: string): string
    local name = id
    for _, k in { "title", "slug" } do
        local v = if type(info) == "table" then info[k] else nil
        if type(v) == "string" and v ~= "" then
            name = v
            break
        end
    end
    local clean = string.lower(name):gsub("[^%w_%-]+", "-"):gsub("%-%-+", "-")
    clean = string.sub(clean, 1, 250):gsub("^%-+", ""):gsub("%-+$", "")
    return `{if clean == "" then id else clean}.json`
end

local function startExport(env: Env, sid: string)
    env.state.export = { kind = "exporting", sessionId = sid }
    task.spawn(function()
        local I18n = env.I18n
        local function failed(err: string?)
            env.state.export = { kind = "idle" }
            env.Toast:Error(
                I18n:T("export.failed.title"),
                if err and err ~= "" then err else I18n:T("export.failed.description")
            )
        end
        local res = env.Server:RequestAsync("GET", `/api/session/{sid}`)
        if not res.ok then
            return failed(res.error)
        end
        local info = json.member(res.text, "data") or res.text
        local messages: { string } = {}
        local cursor: string? = nil
        repeat
            local path = messagesPath(sid)
            if cursor then
                path ..= `&cursor={env.Server:EncodeUri(cursor)}`
            end
            local page = env.Server:RequestAsync("GET", path)
            if not page.ok then
                return failed(page.error)
            end
            for _, item in json.items(json.member(page.text, "data") or "[]") do
                table.insert(messages, item)
            end
            local next = if type(page.json) == "table" and type(page.json.cursor) == "table"
                then page.json.cursor.next
                else nil
            cursor = if type(next) == "string" then next else nil
        until cursor == nil
        local raw = `\{"info":{if info == "" then "{}" else info},"messages":[{table.concat(messages, ",")}]\}`
        local name = filename(if type(res.json) == "table" then res.json.data or res.json else nil, sid)
        local saved = env.Workspace:SaveDownloadAsync(name, json.pretty(raw))
        env.state.export = { kind = "idle" }
        if saved.error then
            env.Toast:Error(I18n:T("export.failed.title"), saved.error)
        else
            env.Toast:Info(I18n:T("export.success.title"), I18n:T("export.success.description", { filename = name }))
        end
    end)
end

local function rawMessages(
    env: Env,
    view: View,
    sid: string,
    raws: { Raw },
    generation: number,
    exporting: boolean,
    look: string
): ui.Node
    local disabled = exporting or sid == ""
    local words = env.I18n:T("export.session")
    local export = ui.row(EXPORT, {
        label = words,
        opacity = if disabled then 0.5 else 1,
        onPress = if disabled
            then nil
            else view:Handler(function()
                startExport(env, sid)
            end),
    }, {
        ui.icon("download", EXPORT_ICON),
        ui.text(words, EXPORT_TEXT),
    })
    local head = ui.row(RAW_HEAD, {
        ui.text(env.I18n:T("rawMessages.title"), HEADING),
        export,
    })
    local open = env.state.open
    -- Each row's own version: whether it shows its JSON. The list's moves with the messages and
    -- the look, so opening one row rebuilds that row alone.
    local opened: { boolean } = table.create(#raws)
    for i, m in raws do
        opened[i] = open[m.id] == true
    end
    local function row(i: number): ui.Node
        local m = raws[i]
        local id, kind, isOpen = m.id, m.kind, opened[i]
        local last = i == #raws
        local created = if m.value.time then m.value.time.created else nil
        local header = ui.row(RAW_ROW, {
            key = `raw-{id}`,
            label = `{kind} {id}`,
            corners = RAW_CORNERS[if i == 1 then 1 else 0][if last and not isOpen then 1 else 0],
            onPress = view:Handler(function()
                open[id] = (if open[id] then nil else true) :: any
                env.state.toggles += 1
            end),
        }, {
            ui.text("{1} ", RAW_KIND, kind),
            ui.text("\u{2022} {1}", RAW_ID, id),
            ui.text(if type(created) == "number" then env.I18n:Date(created, "dateTime") else "-", RAW_TIME),
            ui.icon("chevron-grabber-vertical", RAW_CHEVRON),
        })
        local body = if isOpen
            then ui.col(
                if last then RAW_BODY_LAST else RAW_BODY,
                { key = `raw-body-{id}` },
                { ui.text(json.pretty(m.text), RAW_JSON) }
            )
            else false
        -- The list keys a row's root, so the row's own key sits a level down.
        return ui.col(if i > 1 then RAW_WRAP else RAW_WRAP_FIRST, { header, body } :: { any })
    end
    return ui.col(RAW_SECTION, {
        head,
        -- Only the rows the scroll shows are built, as they come into view.
        ui.virtualList({
            key = "context-raw",
            scrollY = false,
            gap = -1,
            w = "full",
            count = #raws,
            estimatedRowHeight = 32,
            version = `{generation}:{look}`,
            rowVersions = opened,
            renderRow = row,
        }),
    })
end

function tab.panel(env: Env, view: View, props: PanelProps): ui.Node?
    local sid = props.sessionId
    local session = env.Sessions:Get(sid)
    local res = env.Server:Resource(messagesPath(sid), if session then session.version else 0)
    local body = if res.status == "error" then "" else res.text or ""
    local wide = props.width >= 512
    local export = env.state.export
    local exporting = export.kind == "exporting" and export.sessionId == sid
    local look = `{env.Theme:Current().id}:{env.Theme:IsDark()}:{env.I18n:Locale().id}`
    local version =
        `{session and session.version}:{res.generation}:{env.state.toggles}:{exporting}:{wide}:{math.round(props.width)}:{props.placement}:{look}`
    return view:Memo(`panel-{sid}`, version, function()
        -- Split each answer once: a rebuild for anything else reuses it.
        local cached = env.state.parsed[sid]
        local raws: { Raw }
        if cached and cached.generation == res.generation then
            raws = cached.raws
        else
            local texts = json.items(json.member(body, "data") or "[]")
            local values = if type(res.json) == "table" and type(res.json.data) == "table" then res.json.data else {}
            raws = {}
            for i, text in texts do
                local value = values[i] or {}
                table.insert(raws, { text = text, value = value, id = tostring(value.id or ""), kind = tostring(value.type or "") })
            end
            env.state.parsed[sid] = { generation = res.generation, raws = raws }
        end
        -- The figures and the system prompt move only with the session and its messages: a
        -- rebuild for a raw message opening or an export reuses them.
        local prompt = promptOf(raws)
        local figures = `{session and session.version}:{res.generation}:{wide}:{look}`
        local content = ui.col(CONTENT, {
            view:Memo(`stats-{sid}`, figures, function()
                return stats(env, session, sid, raws, wide)
            end),
            if prompt
                then view:Memo(`system-{sid}`, figures, function()
                    return systemPrompt(env, prompt :: string)
                end)
                else false,
            rawMessages(env, view, sid, raws, res.generation, exporting, look),
        } :: { any })
        -- The side region's tab sits 8 under the strip; a narrow window's view has no strip.
        return ui.col(if props.placement == "mobile" then TAB_MOBILE else TAB, {
            ui.col(SCROLL, { content }),
        })
    end)
end

return tab
