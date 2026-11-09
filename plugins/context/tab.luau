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
type Raw = { text: string, value: any }

export type State = {
    -- Raw message rows shown open, by message id.
    open: { [string]: boolean },
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

local TEXT = { size = 13, weight = "regular" :: FontWeight }

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
        tokens = { input = n(t.input), output = n(t.output), reasoning = n(t.reasoning), read = n(cache.read), write = n(cache.write) }
        local m = last.value.model or {}
        provider, model = tostring(m.providerID or ""), tostring(m.id or "")
    elseif #raws == 0 and session and session.usage then
        local u = session.usage
        tokens = { input = u.input, output = u.output, reasoning = u.reasoning, read = u.cacheRead, write = u.cacheWrite }
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
    local rows: { { string } } = {
        { I18n:T("stats.session"), title },
        { I18n:T("stats.messages"), I18n:Number(messages) },
        { I18n:T("stats.provider"), providerName },
        { I18n:T("stats.model"), modelName },
        { I18n:T("stats.limit"), num(limit) },
        { I18n:T("stats.totalTokens"), num(total) },
        { I18n:T("stats.usage"), if usage then I18n:Number(usage / 100, { style = "percent" }) else "-" },
        { I18n:T("stats.inputTokens"), num(tokens and tokens.input) },
        { I18n:T("stats.outputTokens"), num(tokens and tokens.output) },
        { I18n:T("stats.reasoningTokens"), num(tokens and tokens.reasoning) },
        { I18n:T("stats.cacheTokens"), `{num(tokens and tokens.read)} / {num(tokens and tokens.write)}` },
        { I18n:T("stats.userMessages"), I18n:Number(count("user")) },
        { I18n:T("stats.assistantMessages"), I18n:Number(count("assistant")) },
        { I18n:T("stats.totalCost"), I18n:Number(if session then session.cost else 0, { style = "currency" }) },
        { I18n:T("stats.sessionCreated"), time(session and session.created) },
        { I18n:T("stats.lastActivity"), time(last and last.value.time and last.value.time.created) },
    }
    local function stat(pair: { string }): ui.Node
        return ui.col({ gap = 4, w = 0, grow = 1, minW = 0 }, {
            ui.text(pair[1], { size = TEXT.size, weight = TEXT.weight, lineHeight = 19, color = "text-weak" }),
            -- Two 19.5 lines on whole pixels: a row is 59 tall, as Desktop's.
            ui.text(pair[2], { size = 13, weight = "medium", lineHeight = 20, color = "text-strong", selectable = true }),
        })
    end
    local perRow = if wide then 2 else 1
    local lines: { ui.Node } = {}
    for i = 1, #rows, perRow do
        local cells: { ui.Node } = {}
        for j = i, math.min(i + perRow - 1, #rows) do
            table.insert(cells, stat(rows[j]))
        end
        table.insert(lines, ui.row({ w = "full", gap = 16 }, cells))
    end
    return ui.col({ key = "context-stats", w = "full", gap = 16 }, lines)
end

local function systemPrompt(env: Env, raws: { Raw }): ui.Node?
    local prompt: string? = nil
    for i = #raws, 1, -1 do
        if raws[i].value.type == "system" then
            local text = raws[i].value.text
            prompt = if type(text) == "string" then text:match("^%s*(.-)%s*$") else nil
            break
        end
    end
    if prompt == nil or prompt == "" then
        return nil
    end
    return ui.col({ key = "context-system", w = "full", gap = 8 }, {
        ui.text(env.I18n:T("systemPrompt.title"), { size = 13, weight = "regular", lineHeight = 19.5, color = "text-weak" }),
        ui.col({ w = "full", px = 12, py = 8, radius = 6, border = "border-base", fill = "surface-1" }, {
            ui.text(prompt, { size = 13, weight = "regular", lineHeight = 19.5, wrap = true, selectable = true, color = "text-strong" }),
        }),
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
            env.Toast:Error(I18n:T("export.failed.title"), if err and err ~= "" then err else I18n:T("export.failed.description"))
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
            local next = if type(page.json) == "table" and type(page.json.cursor) == "table" then page.json.cursor.next else nil
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

local function rawMessages(env: Env, view: View, sid: string, raws: { Raw }, exporting: boolean): ui.Node
    local disabled = exporting or sid == ""
    local export = ui.row({
        key = "context-export",
        role = "button",
        label = env.I18n:T("export.session"),
        h = 24,
        px = 8,
        gap = 6,
        align = "center",
        radius = 4,
        hover = "surface-hover",
        press = "surface-press",
        hoverTransition = "instant",
        hoverGroup = true,
        opacity = if disabled then 0.5 else 1,
        onPress = if disabled then nil else view:Handler(function()
            startExport(env, sid)
        end),
    }, {
        ui.icon("download", { size = 14, color = "text-weak", hoverGroupColor = "text-strong" }),
        ui.text(env.I18n:T("export.session"), { size = 13, weight = "medium", lineHeight = 20, tracking = -0.04, tabular = true, color = "text-weak", hoverGroupColor = "text-strong" }),
    })
    local head = ui.row({ w = "full", align = "center", justify = "between" }, {
        ui.text(env.I18n:T("rawMessages.title"), { size = 13, weight = "regular", lineHeight = 19.5, color = "text-weak" }),
        export,
    })
    local open = env.state.open
    local list: { ui.Node } = {}
    local lg = 8
    for i, m in raws do
        local id = tostring(m.value.id or "")
        local kind = tostring(m.value.type or "")
        local isOpen = open[id] == true
        local last = i == #raws
        local top = if i == 1 then lg else 0
        local bottom = if last and not isOpen then lg else 0
        local created = if m.value.time then m.value.time.created else nil
        table.insert(
            list,
            ui.row({
                key = `raw-{id}`,
                role = "button",
                label = `{kind} {id}`,
                w = "full",
                h = 32,
                px = 12,
                gap = 8,
                align = "center",
                corners = { top, top, bottom, bottom },
                border = "border-base",
                fill = "bg-base",
                hover = "surface-hover",
                hoverTransition = "instant",
                mt = if i > 1 then -1 else nil,
                onPress = view:Handler(function()
                    open[id] = if open[id] then nil else true
                end),
            }, {
                ui.row({ shrink = true, minW = 0, align = "center" }, {
                    ui.text(`{kind} `, { size = 13, weight = "regular", lineHeight = 19.5, color = "text-strong", noWrap = true }),
                    ui.text(`\u{2022} {id}`, { size = 13, weight = "regular", lineHeight = 19.5, color = "text-base", truncate = true, shrink = true }),
                }),
                ui.spacer(),
                ui.row({ gap = 12, align = "center" }, {
                    ui.text(if type(created) == "number" then env.I18n:Date(created, "dateTime") else "-", { size = 13, weight = "regular", lineHeight = 19.5, color = "text-weak", noWrap = true }),
                    ui.icon("chevron-grabber-vertical", { size = 14, color = "text-weak" }),
                }),
            })
        )
        if isOpen then
            local corner = if last then lg else 0
            table.insert(
                list,
                ui.col({ key = `raw-body-{id}`, w = "full", pad = 12, corners = { 0, 0, corner, corner }, border = "border-base", mt = -1, fill = "bg-base" }, {
                    ui.text(json.pretty(m.text), { size = 12, weight = "regular", lineHeight = 18, mono = true, wrap = true, selectable = true, color = "text-strong" }),
                })
            )
        end
    end
    return ui.col({ key = "context-raw-section", w = "full", gap = 8 }, {
        head,
        ui.col({ key = "context-raw", w = "full" }, list),
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
    local openIds: { string } = {}
    for id in env.state.open do
        table.insert(openIds, id)
    end
    table.sort(openIds)
    local version = table.concat({
        tostring(session and session.version),
        tostring(res.generation),
        table.concat(openIds, ","),
        tostring(exporting),
        tostring(wide),
        tostring(math.round(props.width)),
        env.Theme:Current().id,
        tostring(env.Theme:IsDark()),
        env.I18n:Locale().id,
    }, ":")
    return view:Memo(`panel-{sid}`, version, function()
        local texts = json.items(json.member(body, "data") or "[]")
        local values = if type(res.json) == "table" and type(res.json.data) == "table" then res.json.data else {}
        local raws: { Raw } = {}
        for i, text in texts do
            table.insert(raws, { text = text, value = values[i] or {} })
        end
        local content = ui.col({ key = "session-usage-content", w = "full", pl = 24, pr = 32, pt = 16, pb = 40, gap = 40 }, {
            stats(env, session, sid, raws, wide),
            systemPrompt(env, raws) or false,
            rawMessages(env, view, sid, raws, exporting),
        } :: { any })
        return ui.col({ key = "context-tab", w = "full", h = "full", pt = 8 }, {
            ui.col({ key = "context-scroll", w = "full", grow = 1, minH = 0, scrollY = true }, { content }),
        })
    end)
end

return tab
