--!strict
-- A turn's web searches and page fetches (`session-ui/src/sources/sources.ts`): the steps its
-- assistant messages ran, in order, and the distinct pages they met.

local turn = {}

-- A page: its URL, its title when the search named one, its site (the host without `www.`) and
-- its host.
export type Source = { url: string, title: string?, site: string, host: string }

export type StepStatus = "running" | "completed" | "error"

export type Step =
    { kind: "search", id: string, query: string, status: StepStatus, results: { Source }, error: string? }
    | { kind: "fetch", id: string, source: Source, path: string, status: StepStatus, error: string? }

export type TurnSources = { steps: { Step }, sources: { Source } }

-- The assistant messages of the turn user message `from` opened: up to the next user or shell
-- message (`turnMessages`).
function turn.messages(messages: { any }, from: string): { any }
    local out = {}
    local start: number? = nil
    for i, m in messages do
        if type(m) == "table" and m.id == from then
            start = i
            break
        end
    end
    if start == nil then
        return out
    end
    for i = start + 1, #messages do
        local m = messages[i]
        if type(m) ~= "table" or m.type == "user" or m.type == "shell" then
            break
        end
        if m.type == "assistant" then
            table.insert(out, m)
        end
    end
    return out
end

local function status(tool: any): StepStatus?
    local state = if type(tool.state) == "table" then tool.state else {}
    local s = state.status
    if s == "streaming" or s == nil then
        return nil
    elseif s == "running" then
        return "running"
    elseif s == "completed" then
        return "completed"
    end
    return "error"
end

-- `currentToolInput`: a running or finished tool's input object.
local function input(tool: any): any
    local i = tool.state.input
    if type(i) == "string" then
        return nil
    end
    return if type(i) == "table" then i else {}
end

-- `currentToolOutput`: a running tool's streamed output, or a finished one's text content.
local function output(tool: any): string
    local state = tool.state
    if state.status == "running" then
        local meta = if type(state.metadata) == "table" then state.metadata else {}
        return if type(meta.output) == "string" then meta.output else ""
    end
    local texts: { string } = {}
    if type(state.content) == "table" then
        for _, c in state.content do
            if type(c) == "table" and c.type == "text" and type(c.text) == "string" then
                table.insert(texts, c.text)
            end
        end
    end
    return table.concat(texts, "\n")
end

local function errorOf(tool: any): string?
    if tool.state.status ~= "error" then
        return nil
    end
    local e = tool.state.error
    return if type(e) == "table" and type(e.message) == "string" then e.message else ""
end

-- A web page's parts, or nil for anything but an `http(s)` URL.
function turn.source(url: string): Source?
    local rest = url:match("^https://(.*)$") or url:match("^http://(.*)$")
    if rest == nil then
        return nil
    end
    local authority = rest:match("^([^/?#]*)") or ""
    local host = authority:match("([^@]*)$") or ""
    if host:sub(1, 1) == "[" then
        host = (host:match("^(%[[^%]]*%])") or "")
    else
        host = host:match("^([^:]*)") or ""
    end
    if host == "" then
        return nil
    end
    host = string.lower(host)
    local site = if host:sub(1, 4) == "www." then host:sub(5) else host
    return { url = url, title = nil, site = site, host = host }
end

-- The websearch tool writes each result as a `## [title](url)` heading; a result without a title
-- repeats its URL.
local function results(text: string): { Source }
    local out: { Source } = {}
    for line in (text .. "\n"):gmatch("([^\n]*)\n") do
        if line:sub(1, 4) == "## [" and line:sub(-1) == ")" then
            local split: number? = nil
            local from = 1
            while true do
                local at = line:find("](", from, true)
                if at == nil then
                    break
                end
                split = at
                from = at + 1
            end
            if split and split >= 5 then
                local result = turn.source(line:sub(split + 2, -2))
                if result then
                    local title = (line:sub(5, split - 1):match("^%s*(.-)%s*$") or "")
                    result.title = if title ~= "" and title ~= result.url then title else nil
                    table.insert(out, result)
                end
            end
        end
    end
    return out
end

-- The path and query after the host: `/a/b?c` (a trailing slash dropped, the fragment too).
local function pathAndQuery(url: string): (string, string)
    local rest = url:match("://(.*)$") or url
    local at = rest:find("[/?#]")
    local after = if at then rest:sub(at) else ""
    local beforeFragment = after:match("^([^#]*)") or ""
    local path, query = beforeFragment:match("^([^?]*)%?(.*)$")
    if path == nil then
        path, query = beforeFragment, ""
    end
    return (path :: string):gsub("/+$", ""), if query ~= "" then `?{query}` else ""
end

-- One source per page: the scheme, `www.`, a trailing slash and the fragment don't make another.
local function identity(url: string): string
    local s = turn.source(url)
    local path, query = pathAndQuery(url)
    return `{if s then s.site else ""}{path}{query}`
end

local function step(content: any): Step?
    if type(content) ~= "table" or content.type ~= "tool" or type(content.state) ~= "table" then
        return nil
    end
    local st = status(content)
    if st == nil then
        return nil
    end
    local id = if type(content.id) == "string" then content.id else ""
    local args = input(content) or {}
    if content.name == "websearch" then
        return {
            kind = "search",
            id = id,
            query = if type(args.query) == "string" then args.query else "",
            status = st,
            results = if st == "completed" then results(output(content)) else {},
            error = errorOf(content),
        } :: any
    elseif content.name == "webfetch" then
        local s = if type(args.url) == "string" then turn.source(args.url) else nil
        if s == nil then
            return nil
        end
        local path, query = pathAndQuery(s.url)
        return {
                kind = "fetch",
                id = id,
                source = s,
                path = path .. query,
                status = st,
                error = errorOf(content),
            } :: any
    end
    return nil
end

function turn.sources(messages: { any }): TurnSources
    local steps: { Step } = {}
    for _, m in messages do
        if type(m.content) == "table" then
            for _, c in m.content do
                local s = step(c)
                if s then
                    table.insert(steps, s)
                end
            end
        end
    end
    local seen: { [string]: boolean } = {}
    local sources: { Source } = {}
    local function add(s: Source)
        local key = identity(s.url)
        if not seen[key] then
            seen[key] = true
            table.insert(sources, s)
        end
    end
    for _, s in steps do
        if s.kind == "search" then
            for _, r in s.results do
                add(r)
            end
        elseif s.status ~= "error" then
            add(s.source)
        end
    end
    return { steps = steps, sources = sources }
end

return turn
