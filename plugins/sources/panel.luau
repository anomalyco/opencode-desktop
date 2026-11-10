--!strict
-- A turn's Sources tab (`panel.tsx`, `session-ui/src/sources/sources-panel.tsx`, `sources.css`):
-- its searches, each opening to the results it found, and the pages it opened, in the order they
-- ran; a result or page opens in the browser. Its label is the globe, Sources and the count.

local ui = require("@opencode/ui")
local turn = require("./turn")

local panel = {}

-- A search step's results (`sources-expand-machine.ts`): closed or open. The host's layout
-- motion carries the height between them.
export type Expand = "closed" | "open"

export type State = {
    expand: { [string]: Expand },
    -- Each session's turns as last read, split once per answer.
    parsed: { [string]: { generation: number, messages: { any } } },
}

export type Env = {
    Sessions: SessionsService,
    Server: ServerService,
    Links: LinksService,
    I18n: I18nService,
    Theme: ThemeService,
    Images: ImagesService,
    Diagnostics: DiagnosticsService,
    state: State,
}

-- How a site shows (Knobs > Experiments > Site icons, `experiment.ts`): its letter, or its own
-- favicon over a quiet tile, which the site learns of, so letters are the default.
export type IconMode = "letters" | "site"

-- The project avatar colours, so a site's tile reads like the app's other letter tiles.
local COLORS: { Color } = {
    "avatar-orange",
    "avatar-yellow",
    "avatar-cyan",
    "avatar-green",
    "avatar-red",
    "avatar-pink",
    "avatar-blue",
    "avatar-purple",
}

-- FNV-1a over the site's code points, as Desktop's `hash` with `Math.imul`.
local function hash(s: string): number
    local h = 2166136261
    for _, cp in utf8.codes(s) do
        h = bit32.bxor(h, cp)
        local lo, hi = h % 65536, h // 65536
        h = (lo * 16777619 + ((hi * 16777619) % 65536) * 65536) % 4294967296
    end
    return h
end

-- The session's messages as the server has them, read again whenever the session changes.
local function sourcesOf(env: Env, session: string, from: string): (turn.TurnSources, number)
    local s = env.Sessions:Get(session)
    local version = if s then s.version else 0
    local path = `/api/session/{session}/message?limit=200&order=asc`
    local res = env.Server:Resource(path, version)
    local cached = env.state.parsed[session]
    local messages: { any }
    if cached and cached.generation == res.generation then
        messages = cached.messages
    else
        local json = res.json
        messages = if type(json) == "table" and type(json.data) == "table" then json.data else {}
        env.state.parsed[session] = { generation = res.generation, messages = messages }
    end
    return turn.sources(turn.messages(messages, from)), res.generation
end

function panel.label(env: Env, view: View, props: PanelTabLabelProps): ui.Node?
    local sources, generation = sourcesOf(env, props.sessionId, props.instance or "")
    local count = #sources.sources
    return view:Memo(`sources-label-{props.instance}`, `{count}:{generation}`, function()
        local children: { ui.Node } = {
            ui.icon("globe", { size = 16, color = "text-muted" }),
            ui.text(
                env.I18n:T("title"),
                {
                    size = 13,
                    weight = "regular",
                    lineHeight = 20,
                    color = "text-muted",
                    truncate = true,
                    shrink = true,
                    minW = 0,
                }
            ),
        }
        if count > 0 then
            table.insert(
                children,
                ui.text("\u{b7}", { size = 13, weight = "regular", lineHeight = 20, color = "text-faint" })
            )
            table.insert(
                children,
                ui.text(
                    env.I18n:Number(count),
                    { size = 13, weight = "regular", lineHeight = 20, tabular = true, color = "text-faint" }
                )
            )
        end
        return ui.row({ shrink = true, minW = 0, gap = 6, align = "center" }, children)
    end)
end

-- A site's letter tile (`SiteIcon` in letters mode): 16 round, on the avatar colour its name
-- hashes to, the letter at 9/530 in the avatar ink, a hairline inside.
local function letterTile(site: string): ui.Node
    local color = COLORS[hash(site) % #COLORS + 1]
    local first = ""
    for _, cp in utf8.codes(site) do
        first = utf8.char(cp):upper()
        break
    end
    return ui.row(
        { w = 16, h = 16, center = true, radius = 8, fill = color, border = "avatar-ring", borderWidth = 0.5 } :: any,
        {
            ui.text(first, { size = 9, weight = "medium", lineHeight = 9, color = "avatar-text" }),
        }
    )
end

-- The favicon a site serves at its root, as Desktop asks the site itself (`site-icon.tsx:27`).
local function favicon(host: string): string
    return `https://{host}/favicon.ico`
end

local FAVICON = table.freeze({
    w = 16,
    h = 16,
    radius = 8,
    fit = "cover" :: ImageFit,
    placeholder = "surface-2" :: Color,
    fadeIn = true,
    border = "avatar-ring",
    borderWidth = 0.5,
})

-- A site's icon (`SiteIcon`, `site-icon.tsx:11-44`, `sources.css` `site-icon`): in letters mode
-- its letter tile; in site mode its favicon covering the tile, on layer-02 while it loads
-- (`[data-state]`) and fading in over 150 ms once it has, and the letter tile in its place once
-- it failed. A failed favicon stays failed for the window, so later tiles of that host show the
-- letter at once (`failed`).
local function siteIcon(env: Env, site: string, host: string, mode: IconMode): ui.Node
    if mode ~= "site" then
        return letterTile(site)
    end
    return ui.image(FAVICON, { image = env.Images:Load(favicon(host)) }, letterTile(site))
end

local function labelText(words: string): ui.Node
    return ui.text(words, { size = 12, weight = "regular", lineHeight = 16, tracking = 0.05, color = "text-faint" })
end

local function errorText(err: string?): ui.Node?
    return if err
        then ui.text(err, { size = 12, weight = "regular", lineHeight = 16, color = "status-danger", wrap = true })
        else nil
end

-- A row's three columns (`grid-template-columns: 16px minmax(0, 1fr) auto`, 10 apart, 8 in).
local function stepRow(
    key: string,
    glyph: string,
    words: ui.Node,
    count: ui.Node?,
    hover: boolean,
    extra: { [string]: any }
): ui.Node
    local props: { [string]: any } = {
        key = key,
        role = "button",
        w = "full",
        pad = 8,
        gap = 10,
        radius = 8,
        hover = if hover then "surface-hover" else nil,
    }
    for k, v in extra do
        props[k] = v
    end
    local children: { ui.Node } = {
        ui.row({ w = 16, h = 16, center = true }, { ui.icon(glyph, { size = 14, color = "text-faint" }) }),
        ui.col({ grow = 1, minW = 0, shrink = true }, { words }),
    }
    if count then
        table.insert(children, count)
    end
    return ui.row(props :: any, children)
end

local function words(lines: { ui.Node? }): ui.Node
    local out: { ui.Node } = {}
    for _, n in lines do
        if n then
            table.insert(out, n)
        end
    end
    return ui.col({ gap = 2 }, out)
end

-- One result: its site's tile, then its title over its site (`sources-result`).
local function result(env: Env, view: View, r: turn.Source, mode: IconMode): ui.Node
    local title = r.title or r.url
    return ui.row({
        key = `sources-result-{r.url}`,
        role = "link",
        label = title,
        help = r.url,
        ml = 26,
        py = 6,
        px = 8,
        gap = 10,
        radius = 8,
        hover = "surface-hover",
        onPress = view:Handler(function()
            env.Links:Open({ href = r.url })
        end),
    }, {
        siteIcon(env, r.site, r.host, mode),
        ui.col({ grow = 1, shrink = true, minW = 0 }, {
            ui.text(
                title,
                { size = 13, weight = "regular", lineHeight = 16, color = "text-chrome", noWrap = true, fadeOut = true }
            ),
            ui.text(
                r.site,
                {
                    size = 12,
                    weight = "regular",
                    lineHeight = 16,
                    tracking = 0.05,
                    color = "text-faint",
                    noWrap = true,
                    fadeOut = true,
                }
            ),
        }),
    })
end

local MONO =
    { size = 13, weight = "regular" :: FontWeight, lineHeight = 16, tracking = -0.04, mono = true, noWrap = true }

local function search(env: Env, view: View, s: any, mode: IconMode): ui.Node
    local I18n = env.I18n
    local open = env.state.expand[s.id] == "open"
    local expandable = #s.results > 0
    local text = words({
        labelText(I18n:T(if s.status == "error" then "searchFailed" else "searched")),
        ui.text(
            s.query,
            {
                size = MONO.size,
                weight = MONO.weight,
                lineHeight = MONO.lineHeight,
                tracking = MONO.tracking,
                mono = true,
                noWrap = true,
                fadeOut = true,
                color = "text-chrome",
            }
        ),
        errorText(s.error),
    })
    local count: ui.Node? = nil
    if s.status == "completed" then
        local n = if expandable then I18n:Number(#s.results) else I18n:T("noResults")
        count = ui.text(
            n,
            {
                size = 12,
                weight = "regular",
                lineHeight = 16,
                tabular = true,
                h = 16,
                color = if open and expandable then "text-chrome" else "text-faint",
            }
        )
    end
    local extra: { [string]: any } = {}
    if expandable then
        local id = s.id
        extra.label = `{I18n:T("searched")} {s.query}`
        extra.onPress = view:Handler(function()
            env.state.expand[id] = if env.state.expand[id] == "open" then "closed" else "open"
        end)
    end
    local step: { ui.Node } = { stepRow(`sources-step-{s.id}`, "magnifying-glass", text, count, expandable, extra) }
    if open and expandable then
        local rows: { ui.Node } = {}
        for _, r in s.results do
            table.insert(rows, result(env, view, r, mode))
        end
        table.insert(step, ui.col({ key = `sources-results-{s.id}`, w = "full", clip = true, pt = 2, pb = 6 }, rows))
    end
    return ui.col({ key = `sources-search-{s.id}`, w = "full" }, step)
end

local function fetch(env: Env, view: View, s: any): ui.Node
    local I18n = env.I18n
    local detail = ui.row({ minW = 0 }, {
        ui.text(
            s.source.site,
            {
                size = MONO.size,
                weight = MONO.weight,
                lineHeight = MONO.lineHeight,
                tracking = MONO.tracking,
                mono = true,
                noWrap = true,
                color = "text-chrome",
            }
        ),
        ui.text(
            s.path,
            {
                size = MONO.size,
                weight = MONO.weight,
                lineHeight = MONO.lineHeight,
                tracking = MONO.tracking,
                mono = true,
                noWrap = true,
                fadeOut = true,
                shrink = true,
                minW = 0,
                color = "text-muted",
            }
        ),
    })
    local text =
        words({ labelText(I18n:T(if s.status == "error" then "openFailed" else "opened")), detail, errorText(s.error) })
    local url = s.source.url
    local row = stepRow(`sources-step-{s.id}`, "window-cursor", text, nil, true, {
        label = `{I18n:T("opened")} {url}`,
        help = url,
        onPress = view:Handler(function()
            env.Links:Open({ href = url })
        end),
    })
    return ui.col({ key = `sources-fetch-{s.id}`, w = "full" }, { row })
end

function panel.render(env: Env, view: View, props: PanelProps): ui.Node?
    local from = props.instance or ""
    local sources, generation = sourcesOf(env, props.sessionId, from)
    local mode: IconMode = if env.Diagnostics:Get("sources.icons") == "site" then "site" else "letters"
    local open: { string } = {}
    -- What the shown favicons are doing, so the panel rebuilds as each lands or fails.
    local icons: { string } = {}
    for _, s in sources.steps do
        if s.kind == "search" and env.state.expand[s.id] == "open" then
            table.insert(open, s.id)
            if mode == "site" then
                for _, r in s.results do
                    table.insert(icons, env.Images:State(env.Images:Load(favicon(r.host))).status)
                end
            end
        end
    end
    local version =
        `{generation}:{table.concat(open, ",")}:{mode}:{table.concat(icons, ",")}:{env.Theme:Current().id}:{env.Theme:IsDark()}:{env.I18n:Locale().id}`
    return view:Memo(`sources-{props.sessionId}-{from}`, version, function()
        local steps: { ui.Node } = {}
        for _, s in sources.steps do
            table.insert(steps, if s.kind == "search" then search(env, view, s, mode) else fetch(env, view, s))
        end
        return ui.col({ key = "session-sources-panel", w = "full", h = "full", fill = "bg-base" }, {
            ui.col({ key = "sources-scroll", w = "full", grow = 1, minH = 0, scrollY = true }, {
                ui.col({ key = "sources-panel", w = "full", gap = 2, pad = 8 }, steps),
            }),
        })
    end)
end

return panel
