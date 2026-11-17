--!strict
-- The browser pane (panel.tsx, bar.css): the bar (back, forward, reload, the address field with
-- its site button, zoom and page actions, the element picker and the options menu), the
-- suggestions under the field, and the page, or the new tab page while the tab is blank.

local ui = require("@opencode/ui")
local address = require("./address")
local history = require("./history")

local panel = {}

-- The new tab page lists this many of the newest visits.
local RECENT = 5

export type Env = {
    device: ((View, BrowserTab, PanelProps) -> ui.Node?)?,
    I18n: I18nService,
    Browser: BrowserService,
    Server: ServerService,
    menuTrigger: boolean,
    -- The window's address editor.
    editor: Editor,
    -- The address machine and each page's report counts.
    state: () -> address.State,
    epoch: (tab: BrowserTab) -> address.Epoch,
    visits: () -> { history.Visit },
    -- The pointer is over the address field.
    hovered: () -> boolean,
    -- The system browser here can open the address: a web page that isn't on the server's
    -- machine only, or a file when the server runs here.
    reachable: (url: string) -> boolean,
    -- Site information is open.
    siteOpen: () -> boolean,
    -- What the pane's controls do.
    act: (what: string, tab: BrowserTab?, arg: any?) -> (),
}

-- `text-13-regular` at `--line-height-compact`, as bar.css sets the field and its rows.
local TEXT = table.freeze({ size = 13, weight = "regular", lineHeight = 16, tracking = -0.04, noWrap = true })

local function text(content: string, color: string, extra: { [string]: any }?): ui.Node
    local props: { [string]: any } = table.clone(TEXT)
    props.color = color
    if extra then
        for k, v in extra do
            props[k] = v
        end
    end
    return ui.text(content, props :: any)
end

-- Desktop's `IconButton variant="ghost-muted" size="large"`: 28 square, its icon 16 in
-- icon-muted, the hover and pressed overlays, half opacity when disabled.
local function large(view: View, key: string, icon: string, tip: string, enabled: boolean, press: () -> (), extra: { [string]: any }?): ui.Node
    local props: { [string]: any } = {
        key = key,
        icon = icon,
        variant = "ghost-muted",
        size = "large",
        label = tip,
        help = tip,
        helpSide = "top",
        disabled = not enabled,
        onPress = if enabled then view:Handler(function()
            press()
        end) else nil,
    }
    if extra then
        for k, v in extra do
            props[k] = v
        end
    end
    return ui.iconButton(props :: any)
end

-- Desktop's `IconButton variant="ghost-muted" size="small"`: 20 square with 4 corners.
-- An open popover's trigger (`[data-expanded]`) keeps the pressed overlay and icon-base.
local function small(view: View, key: string, icon: string, tip: string, enabled: boolean, press: () -> (), expanded: boolean?): ui.Node
    local pageAction = expanded == nil
    local glyph = ui.icon(icon :: any, { size = 16, color = if expanded then "icon-base" else "icon-weak" })
    return ui.row({
        fill = if expanded then "surface-press" else nil,
        key = key,
        role = "button",
        label = tip,
        help = tip,
        helpSide = "top",
        -- bar.css at fd95bf0d21: only page actions cover the gap and field edges.
        w = if pageAction then 26 else 20,
        h = if pageAction then 28 else 20,
        ml = if pageAction then -3 else nil,
        mr = if pageAction then -3 else nil,
        radius = 4,
        center = true,
        hover = if enabled and not pageAction then "surface-hover" else nil,
        press = if enabled and not pageAction then "surface-press" else nil,
        hoverGroup = pageAction and enabled,
        hoverTransition = "instant",
        opacity = if enabled then nil else 0.5,
        onPress = if enabled then view:Handler(function()
            press()
        end) else nil,
    }, if pageAction and enabled then {
        ui.row({ w = 20, h = 20, center = true, radius = 4, revealOnHover = true,
            fill = "surface-hover", press = "surface-press", pin = { left = 3, top = 4 } }, {}),
        glyph,
    } else { glyph })
end

-- The URL at rest: the whole address on one line, its host at full strength and the scheme and
-- path faint around it. An address without a host keeps its path at full strength.
local function display(url: string): ui.Node
    local scheme, host, rest = address.parts(url)
    if host == "" then
        return text(url, "text-chrome", { truncate = true, shrink = true, minW = 0 })
    end
    local items: { ui.Node } = {}
    if scheme ~= "" then
        table.insert(items, text(scheme, "text-faint"))
    end
    table.insert(items, text(host, "text-chrome", { shrink = true, minW = 0, truncate = rest == "" }))
    if rest ~= "" then
        table.insert(items, text(rest, "text-faint", { truncate = true, shrink = true, minW = 0 }))
    end
    return ui.row({ grow = 1, shrink = true, minW = 0, align = "center" }, items)
end

-- `browser-address`: 28 tall, 6 corners, layer-01 (layer-02 under the pointer at rest), the
-- focus outline while editing.
local function field(env: Env, view: View, tab: BrowserTab, bar: address.View, zoom: number): ui.Node
    local url = tab:Url()
    local page = if url == "about:blank" then "" else url
    local site = bar.field ~= "" and not bar.editing
    local shown = page ~= "" and (bar.editing or env.hovered())
    local items: { ui.Node } = {}
    if site then
        -- Site information opens 10 left of the button (`shift={-10}`), from a mark there.
        table.insert(items, ui.box({ key = "browser-site-frame" }, {
            small(view, "browser-site", "outline-sliders", env.I18n:T("site.info"), true, function()
                env.act("site", tab)
            end, env.siteOpen()),
            ui.box({ key = "browser-site-anchor", pin = { left = -10, top = 0, bottom = 0 }, w = 1 }),
        }))
    else
        table.insert(items, ui.icon("outline-globe", { size = 16, color = "icon-weak" }))
    end
    local body: { ui.Node } = {}
    if bar.editing then
        table.insert(body, ui.field({
            key = "browser-address-input",
            editor = env.editor,
            variant = "bare",
            dir = "ltr",
            placeholder = env.I18n:T("address.search"),
            keys = { "ArrowUp", "ArrowDown", "Escape" },
            label = env.I18n:T("address.label"),
            grow = 1,
        }))
        -- The suggestion Enter opens, after the typed text.
        if bar.chosen and #bar.query <= address.HINT_MAX_LENGTH then
            table.insert(body, ui.row({ pin = { left = 0, top = 0, bottom = 0 }, gap = 6, align = "center" }, {
                text(bar.query, "text-muted", { opacity = 0 }),
                text(`- {history.bare(bar.chosen.url)}`, "text-muted", { truncate = true, shrink = true, minW = 0 }),
            }))
        end
    elseif bar.field ~= "" then
        table.insert(body, display(bar.field))
    else
        table.insert(body, text(env.I18n:T("address.search"), "text-faint", { truncate = true, shrink = true, minW = 0 }))
    end
    table.insert(items, ui.box({ key = "browser-address-text", grow = 1, shrink = true, minW = 0, h = "full", align = "center" }, { ui.row({ w = "full", h = "full", align = "center" }, body) }))
    -- Any zoom but 100% stays in sight; a click resets it.
    if page ~= "" and math.abs(zoom - 1) > 0.001 then
        table.insert(items, ui.row({
            key = "browser-address-zoom",
            role = "button",
            label = env.I18n:T("zoom.reset"),
            help = env.I18n:T("zoom.reset"),
            helpSide = "top",
            h = 16,
            px = 4,
            radius = 2,
            border = "border-base",
            borderWidth = 0.5,
            fill = "surface-2",
            align = "center",
            hoverGroup = true,
            onPress = view:Handler(function()
                env.act("zoom", tab, "reset")
            end),
        }, { ui.text(env.I18n:T("zoom.percent", { percent = math.floor(zoom * 100 + 0.5) }), { size = 11, weight = "medium", lineHeight = 12, tracking = 0.05, tabular = true, noWrap = true, color = "text-muted", hoverGroupColor = "text-chrome" }) }))
    end
    if shown then
        table.insert(items, ui.row({ key = "browser-address-actions", gap = 6, align = "center" }, {
            small(view, "browser-page-add", "outline-globe-plus", env.I18n:T("page.add"), true, function()
                env.act("page", tab)
            end),
            small(view, "browser-open-external", "outline-arrow-up-right", env.I18n:T(if env.reachable(page) then "external.open" else "external.unavailable"), env.reachable(page), function()
                env.act("external", tab)
            end),
        }))
    end
    local editingPage = bar.editing and not bar.typed and bar.field ~= ""
    -- An address reads left to right in any window (panel.tsx's `<form dir="ltr">`).
    return ui.row({
        key = "browser-address",
        dir = "ltr",
        grow = 1,
        shrink = true,
        minW = 0,
        h = 28,
        pl = if site then 4 elseif bar.editing then 8 else 8,
        pr = if shown then 4 else 8,
        gap = if site or editingPage then 8 else 6,
        radius = 6,
        align = "center",
        fill = "surface-1",
        hover = if bar.editing then nil else "surface-2",
        hoverTransition = "instant",
        border = if bar.editing then "border-focus" else nil,
        borderWidth = if bar.editing then 1 else nil,
        onHover = view:Handler(function(event)
            env.act("hover", tab, event.entered == true)
        end),
        onPress = if bar.editing then nil else view:Handler(function()
            env.act("edit", tab)
        end),
    }, items)
end

-- The bar: 44 tall, 12 in, 8 between its groups, a hairline under it.
local function bar(env: Env, view: View, tab: BrowserTab, state: address.View): ui.Node
    local loading = tab:Loading()
    local url = tab:Url()
    local page = if url == "about:blank" then "" else url
    local reload: ui.Node
    if loading then
        reload = ui.row({
            key = "browser-reload",
            role = "button",
            label = env.I18n:T("action.stop"),
            help = env.I18n:T("action.stop"),
            helpSide = "top",
            w = 28,
            h = 28,
            radius = 6,
            center = true,
            hover = "surface-hover",
            press = "surface-press",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                tab:Stop()
            end),
        }, { ui.loader({ size = 16 }) })
    else
        reload = large(view, "browser-reload", "outline-rotate-clockwise", env.I18n:T("action.reload"), page ~= "", function()
            tab:Reload()
        end, { keybind = "browser.reload" })
    end
    local picking = tab:Inspecting()
    return ui.col({ key = "browser-bar-frame", w = "full" }, {
        -- Chromium lays the 28 tall controls out 8 under the top of the 43.5 the hairline leaves.
        ui.row({ key = "browser-bar", w = "full", h = 43, pt = 1, px = 12, gap = 8, align = "center" }, {
            ui.row({ align = "center" }, {
                large(view, "browser-back", "outline-arrow-left", env.I18n:T("common.goBack"), tab:CanGoBack(), function()
                    tab:Back()
                end),
                large(view, "browser-forward", "outline-arrow-right", env.I18n:T("common.goForward"), tab:CanGoForward(), function()
                    tab:Forward()
                end),
                reload,
            }),
            field(env, view, tab, state, tab:Zoom()),
            ui.row({ align = "center", gap = 4 }, {
                if env.device then large(view, "browser-device-toggle", "outline-sliders", env.I18n:T("plugin.browser.command.toggleToolbar"), true, function()
                    env.act("device", tab)
                end) else nil,
                large(view, "browser-inspect", "outline-browser-annotate", env.I18n:T("inspect"), page ~= "", function()
                    tab:Inspect()
                end, { pressed = picking, color = if picking then "accent" else nil, keybind = "browser.inspect" }),
                large(view, "browser-options", "outline-dots", env.I18n:T("options"), true, function()
                    env.act("options", tab)
                end, { menuTrigger = env.menuTrigger }),
            }),
        }),
        -- `border-b-[0.5px]`: one device pixel at half the colour.
        ui.col({ w = "full", h = 1, fill = "border-weak", opacity = 0.5 }),
    })
end

-- A page in the suggestions or the new tab page: its icon, title and address, the query's
-- matches drawn heavier (`PageRow`).
local function marked(content: string, query: string, color: string, extra: { [string]: any }): ui.Node
    local parts = history.highlight(content, query)
    if #parts == 1 then
        return text(content, color, extra)
    end
    local items: { ui.Node } = {}
    for _, part in parts do
        table.insert(items, text(part.text, color, { weight = if part.match then "medium" else "regular" }))
    end
    local props: { [string]: any } = { align = "center", minW = 0, shrink = true, clip = true }
    for k, v in extra do
        props[k] = v
    end
    return ui.row(props :: any, items)
end

function panel.row(view: View, key: string, visit: history.Visit, query: string, width: number, suggestion: boolean, selected: boolean, open: () -> ()): ui.Node
    local textW = math.max(0, width - 16 - 16 - 8)
    local items: { ui.Node } = {}
    if visit.title ~= "" then
        table.insert(items, marked(visit.title, query, "text-chrome", { maxW = textW * 0.6, truncate = true, shrink = true, minW = 0 }))
        table.insert(items, text("-", "text-chrome"))
    end
    table.insert(items, marked(history.bare(visit.url), query, if suggestion then "text-chrome" else "text-muted", { grow = 1, truncate = true, shrink = true, minW = 0 }))
    return ui.row({
        key = key,
        dir = "ltr",
        role = "button",
        label = if visit.title ~= "" then visit.title else visit.url,
        w = "full",
        h = 28,
        px = 8,
        gap = 8,
        radius = 6,
        align = "center",
        fill = if selected then "surface-press" else nil,
        hover = "surface-hover",
        hoverTransition = "instant",
        onPress = view:Handler(function()
            open()
        end),
    }, {
        ui.icon("outline-globe", { size = 16, color = "icon-weak" }),
        ui.row({ grow = 1, shrink = true, minW = 0, gap = 4, align = "center" }, items),
    })
end

-- The new tab page: Open a page and what it's for, top-aligned; or the newest visits.
local function home(env: Env, view: View, tab: BrowserTab, width: number): ui.Node
    local visits = env.visits()
    if #visits == 0 then
        return ui.col({ key = "browser-home", dir = "auto", w = "full", grow = 1, minH = 0, scrollY = true, align = "center" }, {
            ui.col({ w = "full", maxW = 360, px = 24, pt = 80, gap = 16, align = "center" }, {
                ui.icon("outline-globe", { size = 20, color = "icon-weak" }),
                ui.col({ w = "full", gap = 8, align = "center" }, {
                    ui.text(env.I18n:T("home.title"), { size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", textAlign = "center" }),
                    ui.text(env.I18n:T("home.description"), { size = 13, weight = "regular", lineHeight = 16, color = "text-muted", textAlign = "center", wrap = true }),
                }),
            }),
        })
    end
    local listW = math.min(608, width) - 48
    local rows: { ui.Node } = {}
    for n, visit in visits do
        if n > RECENT then
            break
        end
        table.insert(rows, panel.row(view, `browser-recent-{n}`, visit, "", listW, false, false, function()
            env.act("go", tab, visit.url)
        end))
    end
    return ui.col({ key = "browser-home", dir = "auto", w = "full", grow = 1, minH = 0, scrollY = true, align = "center" }, {
        ui.col({ w = "full", maxW = 608, px = 24, pt = 48, pb = 40, gap = 16 }, {
            ui.text(env.I18n:T("home.recent"), { size = 13, weight = "regular", lineHeight = 16, color = "text-muted" }),
            ui.col({ w = "full", gap = 8 }, rows),
        }),
    })
end

-- A page that failed to load or whose renderer crashed (panel.tsx's failed state, over the page):
-- the globe, what happened and what to do, centred above the file empty state's 200 bottom pad.
local function failed(env: Env): ui.Node
    return ui.col({ key = "browser-failed", dir = "auto", w = "full", grow = 1, minH = 0, px = 24, pt = 24, pb = 200, gap = 8, align = "center", justify = "center" }, {
        ui.icon("outline-globe", { size = 20, color = "text-weak", mb = 8 }),
        ui.text(env.I18n:T("failed.title"), { size = 13, weight = "medium", lineHeight = 16, color = "text-strong", textAlign = "center" }),
        ui.text(env.I18n:T("failed.description"), { size = 13, weight = "regular", lineHeight = 20, color = "text-weak", textAlign = "center", wrap = true }),
    })
end

-- The suggestions under the field (`browser-suggestions`): the field's width and 8 more on each
-- side, so their icons and text line up with the field's; 8 in, 12 corners, the floating shadow.
local function suggestions(env: Env, view: View, tab: BrowserTab, state: address.View, width: number): ui.Node?
    if not state.listed then
        return nil
    end
    -- The field starts past the bar's 12 and its three 28 buttons and 8, and ends before 8 and
    -- the 28 + 4 + 28 on the right.
    local fieldW = math.max(0, width - 24 - 84 - 8 - 8 - 60)
    local rows: { ui.Node } = {}
    for n, visit in state.suggestions do
        table.insert(rows, panel.row(view, `browser-suggestion-{n}`, visit, state.query, fieldW, true, n == state.active, function()
            env.act("go", tab, visit.url)
        end))
    end
    return ui.col({
        key = "browser-suggestions",
        role = "list",
        label = env.I18n:T("address.suggestions"),
        pin = { left = 12 + 84 + 8 - 8, top = 8 + 28 + 4 },
        w = fieldW + 16,
        pad = 8,
        radius = 12,
        fill = "bg-base",
        shadow = "floating",
    }, rows)
end

local function alertRow(key: string, words: string): ui.Node
    return ui.col({ key = key, role = "alert", w = "full" }, {
        ui.col({ dir = "auto", w = "full", px = 12, py = 6 }, {
            ui.text(words, { size = 12, weight = "regular", lineHeight = 16, color = "status-danger", wrap = true }),
        }),
        ui.divider(),
    })
end

-- Refused navigation notice (`panel.tsx:713-721`, `model.ts:121-131`), shown while keeping the page.
local function refusal(env: Env, tab: BrowserTab): ui.Node?
    if tab:LoadError() ~= nil then
        return nil
    end
    local code = tab:Refusal()
    if code == nil then
        return nil
    end
    local words = if code == "browser.address.credentials"
        then env.I18n:T("refused.credentials")
        elseif code == "browser.address.workspace" then env.I18n:T("refused.workspace")
        elseif code == "browser.address.web" then env.I18n:T("refused.web")
        else env.I18n:T("common.requestFailed")
    return alertRow("browser-refusal", words)
end

-- The page's last download, when it failed: one line under the bar, as Desktop's published error
-- (panel.tsx's `role="alert"` row: 12 regular, the danger colour, 12 and 6 in, a muted rule
-- under it). A successful download shows nothing there, as Desktop's doesn't: it's for the
-- agent's tools (`browser_files`).
local function download(env: Env, tab: BrowserTab): ui.Node?
    local files = tab:Files()
    local last = files[#files]
    if last == nil or last.state ~= "failed" then
        return nil
    end
    local words = if last.error == "too_large"
        then env.I18n:T("download.failed.tooLarge", { name = last.name })
        elseif last.error == "canceled" then env.I18n:T("download.failed.canceled", { name = last.name })
        elseif last.error == "too_many" then env.I18n:T("download.failed.tooMany", { name = last.name })
        else env.I18n:T("download.failed.other", { name = last.name, reason = last.error or "failed" })
    return alertRow("browser-download-error", words)
end

-- A tab's pane.
function panel.render(env: Env, view: View, props: PanelProps): ui.Node
    local tab = if props.instance then env.Browser:Get(props.instance) else nil
    if tab == nil then
        return ui.col({ key = "browser-pane", w = "full", h = "full", fill = "bg-base" }, {})
    end
    local state = address.view(env.state(), tab.Id, env.epoch(tab), env.visits())
    local engine = env.Browser:Engine().state
    local items: { ui.Node } = {}
    if engine ~= "unsupported" then
        table.insert(items, bar(env, view, tab, state))
    end
    local toolbar = if env.device and engine ~= "unsupported" then env.device(view, tab, props) else nil
    if toolbar then table.insert(items, toolbar) end
    local alert = refusal(env, tab) or download(env, tab)
    if alert then
        table.insert(items, alert)
    end
    local url = tab:Url()
    local blank = (url == "" or url == "about:blank") and not tab:Loading() and not state.navigating
    -- As Desktop's: the error shows once the load stops, so a reload shows its progress.
    if tab:LoadError() ~= nil and not tab:Loading() then
        table.insert(items, failed(env))
    elseif blank and (engine == "ready" or engine == "starting") then
        table.insert(items, home(env, view, tab, props.width))
    else
        table.insert(items, ui.browserSurface({ key = "browser-page", tab = tab, grow = 1,
            areaWidth = props.width, areaHeight = props.height - 44 - (if toolbar then 36 else 0) }))
    end
    local list = suggestions(env, view, tab, state, props.width)
    if list then
        table.insert(items, list)
    end
    return ui.col({ key = "browser-pane", w = "full", h = "full", minW = 0, fill = "bg-base", clip = true }, items)
end

return panel
