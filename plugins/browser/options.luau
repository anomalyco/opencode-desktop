--!strict
-- Browser '...' options popover and nested "More tools" submenu matching upstream
-- `packages/gui-plugins/src/browser/panel.tsx:1214-1323` (`live-browser-menu-dark.png`,
-- `live-browser-more-tools-dark.png`).

local ui = require("@opencode/ui")

local options = {}

export type MenuData = {
    tabId: string,
    url: string,
    zoom: number,
    inspectable: boolean,
    suspended: boolean,
    failed: boolean,
    serverFile: boolean,
    canOpenExternal: boolean,
    canRevealFile: boolean,
    canViewSource: boolean,
    canFind: boolean,
    canHardReload: boolean,
    canClearHistory: boolean,
}

export type Callbacks = {
    I18n: I18nService,
    act: (action: string, tabId: string, arg: any?) -> (),
}

local ROW_TEXT = table.freeze({ size = 13, weight = "regular", lineHeight = 16, tracking = -0.04, noWrap = true })
local HINT_TEXT = table.freeze({ size = 11, weight = "regular", lineHeight = 16, color = "text-faint", wrap = true })

local function menuItem(
    view: View,
    key: string,
    label: string,
    enabled: boolean,
    press: () -> (),
    commandId: string?
): ui.Node
    local children: { ui.Node } = {
        ui.text(label, ROW_TEXT, { color = if enabled then "text-chrome" else "text-faint", grow = 1, shrink = true, minW = 0, truncate = true }),
    }
    if commandId then
        table.insert(children, ui.keybind({ command = commandId }))
    end
    return ui.row({
        key = key,
        role = "button",
        label = label,
        w = "full",
        h = 28,
        px = 10,
        gap = 12,
        radius = 4,
        align = "center",
        hover = if enabled then "surface-hover" else nil,
        press = if enabled then "surface-press" else nil,
        hoverTransition = "instant",
        opacity = if enabled then nil else 0.5,
        onPress = if enabled then view:Handler(function()
            press()
        end) else nil,
    }, children)
end

local function menuHint(text: string): ui.Node
    return ui.row({ w = "full", px = 10, pb = 6 }, {
        ui.text(text, HINT_TEXT),
    })
end

local function separator(): ui.Node
    return ui.col({ w = "full", h = 1, my = 4, fill = "border-weak", opacity = 0.5 })
end

-- Renders the top-level 220 pt '...' options popover (`live-browser-menu-dark.png`).
function options.renderMain(cb: Callbacks, view: View, d: MenuData): ui.Node
    local hasUrl = d.url ~= ""
    local zoomed = hasUrl and math.abs(d.zoom - 1) > 0.001
    local pct = cb.I18n:T("zoom.percent", { percent = math.floor(d.zoom * 100 + 0.5) })
    local items: { ui.Node } = {}

    if d.canFind then
        table.insert(items, menuItem(view, "browser-menu-find", cb.I18n:T("find.open"), d.inspectable, function()
            cb.act("find.open", d.tabId)
        end, "browser.find"))
    end

    -- Inline Zoom row (`Menu.ItemRow`): "Zoom" on left, `-` / `100%` / `+` inline on right.
    table.insert(items, ui.row({ key = "browser-menu-zoom", w = "full", h = 28, pl = 10, pr = 4, gap = 4, align = "center" }, {
        ui.text(cb.I18n:T("zoom.label"), ROW_TEXT, { color = "text-chrome", grow = 1, shrink = true, minW = 0 }),
        ui.iconButton({
            key = "browser-menu-zoom-out",
            icon = "minus",
            variant = "ghost-muted",
            size = "small",
            label = cb.I18n:T("zoom.out"),
            disabled = not hasUrl,
            onPress = if hasUrl then view:Handler(function()
                cb.act("zoom", d.tabId, "out")
            end) else nil,
        }),
        ui.row({
            key = "browser-menu-zoom-reset",
            role = "button",
            label = cb.I18n:T("zoom.reset"),
            h = 22,
            px = 6,
            radius = 4,
            center = true,
            hover = if zoomed then "surface-hover" else nil,
            press = if zoomed then "surface-press" else nil,
            hoverTransition = "instant",
            onPress = if zoomed then view:Handler(function()
                cb.act("zoom", d.tabId, "reset")
            end) else nil,
        }, {
            ui.text(pct, { size = 12, weight = "regular", lineHeight = 16, tabular = true, noWrap = true, color = if zoomed then "text-chrome" else "text-muted" }),
        }),
        ui.iconButton({
            key = "browser-menu-zoom-in",
            icon = "plus",
            variant = "ghost-muted",
            size = "small",
            label = cb.I18n:T("zoom.in"),
            disabled = not hasUrl,
            onPress = if hasUrl then view:Handler(function()
                cb.act("zoom", d.tabId, "in")
            end) else nil,
        }),
    }))

    local canCopy = hasUrl and not d.serverFile
    table.insert(items, menuItem(view, "browser-menu-copy", cb.I18n:T("link.copy"), canCopy, function()
        cb.act("link.copy", d.tabId)
    end))
    if d.serverFile then
        table.insert(items, menuHint(cb.I18n:T("menu.privateAddress")))
    end

    local canAdd = hasUrl and not d.suspended and not d.failed
    table.insert(items, menuItem(view, "browser-menu-add-page", cb.I18n:T("page.add"), canAdd, function()
        cb.act("page", d.tabId)
    end))

    if d.canViewSource then
        table.insert(items, menuItem(view, "browser-menu-view-source", cb.I18n:T("preview.source"), true, function()
            cb.act("view.source", d.tabId)
        end))
    end

    table.insert(items, separator())
    table.insert(items, ui.flyoutRow({
        key = "browser-menu-more",
        text = cb.I18n:T("menu.more"),
        popover = "more-tools",
        side = "left",
        props = d :: any,
    }))
    table.insert(items, separator())
    table.insert(items, menuItem(view, "browser-menu-settings", cb.I18n:T("settings.open"), true, function()
        cb.act("settings", d.tabId)
    end))

    return ui.col({
        key = "browser-options-menu",
        role = "list",
        label = cb.I18n:T("options"),
        w = 220,
        pad = 4,
        radius = 8,
        fill = "surface-1",
        border = "border-base",
        borderWidth = 0.5,
        shadow = "floating",
    }, items)
end

-- Renders the nested "More tools" submenu (`live-browser-more-tools-dark.png`).
function options.renderMore(cb: Callbacks, view: View, d: MenuData): ui.Node
    local hasUrl = d.url ~= ""
    local items: { ui.Node } = {
        menuItem(view, "browser-more-inspect", cb.I18n:T("inspect"), d.inspectable, function()
            cb.act("inspect", d.tabId)
        end, "browser.inspect"),
        menuItem(view, "browser-more-external", cb.I18n:T("menu.openPage"), d.canOpenExternal, function()
            cb.act("external", d.tabId)
        end),
    }
    if not d.canOpenExternal then
        table.insert(items, menuHint(cb.I18n:T("menu.pageUnavailable")))
    end
    if d.canRevealFile then
        table.insert(items, menuItem(view, "browser-more-reveal", cb.I18n:T("external.reveal"), true, function()
            cb.act("reveal", d.tabId)
        end))
    end
    if d.canHardReload then
        table.insert(items, menuItem(view, "browser-more-hard-reload", cb.I18n:T("menu.hardReload"), hasUrl, function()
            cb.act("hard.reload", d.tabId)
        end, "browser.hardReload"))
    end
    table.insert(items, menuItem(view, "browser-more-clear-history", cb.I18n:T("history.clear"), d.canClearHistory, function()
        cb.act("history.clear", d.tabId)
    end))

    return ui.col({
        key = "browser-more-tools-menu",
        role = "list",
        label = cb.I18n:T("menu.more"),
        minW = 220,
        pad = 4,
        radius = 8,
        fill = "surface-1",
        border = "border-base",
        borderWidth = 0.5,
        shadow = "floating",
    }, items)
end

return options
