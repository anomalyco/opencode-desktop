--!strict
-- Find in page state machine and bar UI matching upstream
-- `packages/gui-plugins/src/browser/find-machine.ts` and `find-bar.tsx`.
-- Per-tab query and match state; ready to connect to WS-1's `FindHandler` capability as soon as
-- the engine advertises `find: true`.

local ui = require("@opencode/ui")

local find = {}

export type State =
    { status: "closed", query: string }
    | { status: "editing", query: string }
    | { status: "searching", query: string, id: number }
    | { status: "found", query: string, active: number, total: number }

export type Event =
    { type: "open" }
    | { type: "close" }
    | { type: "query", query: string }
    | { type: "issued", id: number }
    | { type: "found", id: number, active: number, total: number, finalUpdate: boolean }

export type View = {
    query: string,
    result: { active: number, total: number }?,
}

function find.initial(): State
    return { status = "closed", query = "" }
end

-- Changing or closing the query invalidates any in-flight match result (`find-machine.ts`).
function find.step(state: State, event: Event): State
    if event.type == "open" then
        if state.status == "closed" then
            return { status = "editing", query = state.query }
        end
        return state
    end
    if state.status == "closed" then
        return state
    end
    if event.type == "close" then
        return { status = "closed", query = state.query }
    elseif event.type == "query" then
        return { status = "editing", query = event.query }
    elseif event.type == "issued" then
        return { status = "searching", query = state.query, id = event.id }
    else
        if state.status ~= "searching" or event.id ~= state.id or not event.finalUpdate then
            return state
        end
        return { status = "found", query = state.query, active = event.active, total = event.total }
    end
end

function find.view(state: State): View?
    if state.status == "closed" then
        return nil
    end
    return {
        query = state.query,
        result = if state.status == "found" then { active = state.active, total = state.total } else nil,
    }
end

-- Renders the 36 pt Find in page bar (`find-bar.tsx::FindBar`, `live-browser-find-dark.png`).
function find.render(
    view: View,
    i18n: I18nService,
    editor: Editor,
    current: View,
    onPrev: () -> (),
    onNext: () -> (),
    onClose: () -> ()
): ui.Node
    local hasQuery = current.query ~= ""
    local items: { ui.Node } = {
        ui.icon("magnifying-glass", { size = 14, color = "icon-weak" }),
        ui.field({
            key = "browser-find-input",
            editor = editor,
            variant = "bare",
            dir = "auto",
            placeholder = i18n:T("find.open"),
            label = i18n:T("find.open"),
            keys = { "Escape" },
            grow = 1,
            ml = 4,
        }),
    }
    if current.result ~= nil then
        local countKey = if current.result.total == 1 then "find.matches.one" else "find.matches.other"
        local countText = i18n:T(countKey, { active = current.result.active, count = current.result.total })
        table.insert(items, ui.text(countText, {
            key = "browser-find-count",
            role = "status",
            px = 4,
            size = 12,
            weight = "regular",
            lineHeight = 16,
            tabular = true,
            noWrap = true,
            color = "text-muted",
        }))
    end
    table.insert(items, ui.iconButton({
        key = "browser-find-prev",
        icon = "chevron-up",
        variant = "ghost-muted",
        size = "small",
        label = i18n:T("find.previous"),
        help = i18n:T("find.previous"),
        disabled = not hasQuery,
        onPress = if hasQuery then view:Handler(function()
            onPrev()
        end) else nil,
    }))
    table.insert(items, ui.iconButton({
        key = "browser-find-next",
        icon = "chevron-down",
        variant = "ghost-muted",
        size = "small",
        label = i18n:T("find.next"),
        help = i18n:T("find.next"),
        disabled = not hasQuery,
        onPress = if hasQuery then view:Handler(function()
            onNext()
        end) else nil,
    }))
    table.insert(items, ui.iconButton({
        key = "browser-find-close",
        icon = "close",
        variant = "ghost-muted",
        size = "small",
        label = i18n:T("find.close"),
        help = i18n:T("find.close"),
        onPress = view:Handler(function()
            onClose()
        end),
    }))
    return ui.col({ key = "browser-find-bar", role = "region", label = i18n:T("find.open"), w = "full" }, {
        ui.row({ w = "full", h = 35, px = 12, gap = 4, align = "center", fill = "surface-1" }, items),
        ui.col({ w = "full", h = 1, fill = "border-weak" }),
    })
end

return find
