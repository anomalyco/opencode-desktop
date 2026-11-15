--!strict
-- The Studio console, as Roblox Studio's Output and command bar: every plugin's `print`, `warn`,
-- errors and reloads, newest last, an error as a card with its source location (a link that
-- opens the file at that line) and its traceback; the target plugin's live costs; and a command
-- bar that runs Luau in the target plugin's VM.

local ui = require("@opencode/ui")

local console = {}

export type State = {
    -- The plugin the command bar runs in.
    target: string?,
}

export type Env = {
    Console: ConsoleService,
    Diagnostics: DiagnosticsService,
    Links: LinksService,
    I18n: I18nService,
    editor: Editor,
    state: State,
    refresh: () -> (),
    -- This plugin's id: an error in another plugin's VM lands in the console on its own.
    own: string,
}

local MONO = { size = 11, weight = "regular" :: FontWeight, lineHeight = 16, mono = true }

local function summary(env: Env, lines: { ConsoleLine }): string
    local errors = 0
    for _, l in lines do
        if l.level == "error" then
            errors += 1
        end
    end
    return env.I18n:T("console.summary", {
        lines = env.I18n:T("console.lines", { count = #lines }),
        errors = env.I18n:T("console.errors", { count = errors }),
    })
end

local function card(env: Env, view: View, n: number, l: ConsoleLine): ui.Node
    local ink: Color, fill: Color, border: Color = "text-muted", "surface-1", "border-weak"
    if l.level == "error" then
        ink, fill, border = "status-danger", "surface-1", "status-danger"
    elseif l.level == "warn" then
        ink = "status-warning"
    end
    local first, rest = l.message:match("^([^\n]*)\n?(.*)$")
    local head: { ui.Node } = { ui.text(l.plugin, { size = 12, weight = "regular", lineHeight = 16, color = ink }) }
    if l.file and l.line then
        local at = `{l.file}:{l.line}`
        local href = l.href
        table.insert(
            head,
            ui.row({
                key = `luau-console-at-{n}`,
                role = if href then "link" else nil,
                label = at,
                onPress = if href
                    then view:Handler(function()
                        env.Links:Open({ href = href })
                    end)
                    else nil,
            }, {
                ui.text(at, { size = 12, weight = "regular", lineHeight = 16, color = "text-interactive" }),
            })
        )
    end
    if l.count > 1 then
        table.insert(
            head,
            ui.text(`×{l.count}`, { size = 12, weight = "regular", lineHeight = 16, color = "text-faint" })
        )
    end
    local body: { ui.Node } = {
        ui.row({ w = "full", gap = 8, align = "center" }, head),
        ui.text(
            first or l.message,
            { size = 13, weight = "regular", lineHeight = 18, color = "text-strong", wrap = true, selectable = true }
        ),
    }
    local trace = (rest or ""):gsub("\n%s*\n", "\n")
    if trace:match("%S") then
        table.insert(
            body,
            ui.text(trace, {
                size = MONO.size,
                weight = MONO.weight,
                lineHeight = MONO.lineHeight,
                mono = true,
                color = "text-faint",
                wrap = true,
                selectable = true,
            })
        )
    end
    return ui.col(
        { key = `luau-console-{n}`, w = "full", px = 12, py = 8, gap = 4, radius = 6, fill = fill, border = border },
        body
    )
end

-- The target plugin's live costs: its Luau heap, the cost of a render and the memo hit rate.
local function costs(env: Env, target: string): ui.Node
    local m = env.Diagnostics:Metrics(target)
    local words = if m
        then env.I18n:T("console.costs", {
            plugin = target,
            kb = env.I18n:Number(math.round(m.memoryKb)),
            us = env.I18n:Number(math.round(m.renderUs)),
            hits = env.I18n:Number(m.memoHitRate, { style = "percent" }),
        })
        else env.I18n:T("console.stopped", { plugin = target })
    return ui.text(words, {
        size = MONO.size,
        weight = MONO.weight,
        lineHeight = MONO.lineHeight,
        mono = true,
        color = "text-faint",
        truncate = true,
        shrink = true,
        minW = 0,
    })
end

-- Runs the command bar's text in the target plugin and writes the result to the console.
function console.submit(env: Env, text: string)
    local source = text:match("^%s*(.-)%s*$") or ""
    local target = env.state.target
    if source == "" or target == nil then
        return
    end
    env.Console:Log(`> {source}`)
    local result = env.Console:Eval(target, source)
    if result.ok then
        env.Console:Log(result.value or "nil")
    elseif target == env.own then
        env.Console:Error(result.error or "error")
    end
    env.editor:SetText("")
end

function console.render(env: Env, view: View, props: PanelProps): ui.Node?
    local lines = env.Console:Lines()
    local plugins = env.Diagnostics:Plugins()
    local target = env.state.target
    if target == nil or not table.find(plugins, target) then
        target = plugins[1]
        env.state.target = target
    end
    local cards: { ui.Node } = {}
    for n, l in lines do
        table.insert(cards, card(env, view, n, l))
    end
    local head = ui.row({ w = "full", align = "center", gap = 8 }, {
        ui.text(summary(env, lines), { size = 13, weight = "regular", lineHeight = 18, color = "text-faint" }),
        ui.spacer(),
        ui.row({
            key = "luau-console-clear",
            role = "button",
            label = env.I18n:T("console.clear"),
            h = 24,
            px = 8,
            radius = 4,
            center = true,
            hover = "surface-hover",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                env.Console:Clear()
            end),
        }, {
            ui.text(
                env.I18n:T("console.clear"),
                { size = 13, weight = "regular", lineHeight = 18, color = "text-muted" }
            ),
        }),
    })
    local bar: { ui.Node } = {}
    if target then
        local current = target
        table.insert(
            bar,
            ui.row({
                key = "luau-console-target",
                role = "button",
                label = env.I18n:T("console.target", { plugin = current }),
                help = env.I18n:T("console.target.tip"),
                helpSide = "top",
                h = 28,
                px = 8,
                radius = 6,
                center = true,
                border = "border-weak",
                hover = "surface-hover",
                hoverTransition = "instant",
                onPress = view:Handler(function()
                    local i = table.find(plugins, current) or 0
                    env.state.target = plugins[i % #plugins + 1]
                    env.refresh()
                end),
            }, {
                ui.text(current, {
                    size = MONO.size,
                    weight = MONO.weight,
                    lineHeight = MONO.lineHeight,
                    mono = true,
                    color = "text-strong",
                }),
            })
        )
        table.insert(
            bar,
            ui.field({
                key = "luau-console-input",
                editor = env.editor,
                placeholder = env.I18n:T("console.placeholder"),
                grow = 1,
                minW = 0,
            })
        )
    end
    return ui.col({ key = "luau-console", w = "full", h = "full", pt = if props.placement == "mobile" then 0 else 8 }, {
        ui.col(
            { key = "luau-console-scroll", w = "full", grow = 1, minH = 0, scrollY = true, stickBottom = true } :: any,
            {
                ui.col({ w = "full", px = 16, py = 12, gap = 8 }, { head, table.unpack(cards) }),
            }
        ),
        ui.col({ key = "luau-console-bar", w = "full", px = 16, py = 8, gap = 6 }, {
            if target then costs(env, target) else ui.spacer(),
            ui.row({ w = "full", gap = 8, align = "center" }, bar),
        }),
    })
end

return console
