--!strict
-- What a page asks its user, drawn over the page as the app's own dialog (Desktop's
-- `prompts.tsx` `PageDialog` and `LeaveDialog`, with `prompt-machine.ts` `promptSite`): the
-- page's `alert`, `confirm` and `prompt`, and "Leave site?" for its `beforeunload`. The host holds
-- the dialog (`BrowserTab:Dialog`) and its input guard (`armed`); the agent may answer the same
-- dialog with `browser_dialog`, and the first answer wins.

local ui = require("@opencode/ui")
local address = require("./address")

local prompt = {}

-- The pane's machine: the dialog it shows and fills the window's prompt field for, and whether
-- the person ticked "Stop this page from showing more messages".
export type State = { status: "idle" } | { status: "shown", tab: string, id: string, stop: boolean }

export type Event =
    -- A page's dialog now: what `BrowserTab:Dialog` says for page `tab` (nil when none waits).
    { type: "dialog", tab: string, dialog: BrowserDialog? }
    | { type: "stop", value: boolean }
    | { type: "answered" }

export type Effect = { type: "fill", text: string } | { type: "focus" } | { type: "release" }

prompt.initial = { status = "idle" } :: State

function prompt.step(state: State, event: Event): (State, { Effect })
    if event.type == "dialog" then
        local dialog = event.dialog
        if dialog == nil then
            if state.status == "shown" and state.tab == event.tab then
                return { status = "idle" }, { { type = "release" } }
            end
            return state, {}
        end
        if state.status == "shown" and state.tab == event.tab and state.id == dialog.id then
            return state, {}
        end
        local effects: { Effect } = {}
        if dialog.kind == "prompt" then
            table.insert(effects, { type = "fill", text = dialog.defaultText })
            table.insert(effects, { type = "focus" })
        end
        return { status = "shown", tab = event.tab, id = dialog.id, stop = false }, effects
    elseif event.type == "stop" then
        if state.status == "shown" then
            return { status = "shown", tab = state.tab, id = state.id, stop = event.value }, {}
        end
        return state, {}
    end
    return { status = "idle" }, { { type = "release" } }
end

export type Env = {
    I18n: I18nService,
    editor: Editor,
    state: () -> State,
    dispatch: (event: Event) -> (),
}

-- Who asks (`promptSite`): a web page by its host, a workspace file as a file, anything else as
-- the page; a frame of another origin than the tab's page as an embedded page.
local function site(env: Env, origin: string, top: string): string
    local scheme, host = address.parts(origin)
    local topScheme, topHost = address.parts(top)
    local lower = string.lower(scheme)
    local embedded = origin ~= "" and (lower ~= string.lower(topScheme) or string.lower(host) ~= string.lower(topHost))
    if lower == "file://" then
        return env.I18n:T("prompt.site.file")
    end
    if (lower == "http://" or lower == "https://") and host ~= "" then
        return env.I18n:T(if embedded then "prompt.site.embedded" else "prompt.site.web", { host = host })
    end
    return env.I18n:T(if embedded then "prompt.site.embedded.page" else "prompt.site.page")
end

-- Answers the dialog for the person, then lets the pane forget it.
local function answer(env: Env, tab: BrowserTab, dialog: BrowserDialog, accept: boolean)
    local state = env.state()
    local mine = state.status == "shown" and state.tab == tab.Id and state.id == dialog.id
    local text = if dialog.kind == "prompt" then (if mine then env.editor:Text() else dialog.defaultText) else nil
    local stop = state.status == "shown" and mine and state.stop
    tab:AnswerDialog(dialog.id, accept, text, stop)
    if mine then
        env.dispatch({ type = "answered" })
    end
end

-- The dialog over the page, or nil when the page asks nothing.
function prompt.render(env: Env, view: View, tab: BrowserTab): ui.Node?
    local dialog = tab:Dialog()
    if dialog == nil then
        return nil
    end
    local state = env.state()
    local mine = state.status == "shown" and state.tab == tab.Id and state.id == dialog.id
    -- A press already on its way when it appeared can't answer it (`activation-delay.ts`).
    local armed = dialog.armed
    local leave = dialog.kind == "leave"
    local title = if leave then env.I18n:T("prompt.leave.title") else site(env, dialog.origin, tab:Url())
    local body: { ui.Node } = {
        ui.text(title, { size = 14, weight = "medium", lineHeight = 20, color = "text-strong", wrap = true, dir = "auto" }),
    }
    local message = if leave then env.I18n:T("prompt.leave.description") else dialog.message
    if message ~= "" then
        table.insert(body, ui.text(message, { size = 13, weight = "regular", lineHeight = 20, color = "text-base", wrap = true, dir = "auto" }))
    end
    if dialog.kind == "prompt" then
        if mine then
            table.insert(body, ui.field({
                key = "browser-prompt-value",
                editor = env.editor,
                label = env.I18n:T("prompt.value"),
                dir = "auto",
                onSubmit = view:Handler(function()
                    local now = tab:Dialog()
                    if now and now.id == dialog.id and now.armed then
                        answer(env, tab, dialog, true)
                    end
                end),
            }))
        else
            table.insert(body, ui.text(dialog.defaultText, { size = 13, lineHeight = 20, color = "text-weak", wrap = true, dir = "auto" }))
        end
    end
    if dialog.repeated and not leave then
        table.insert(body, ui.row({ key = "browser-prompt-stop", gap = 8, align = "center" }, {
            ui.switch({
                key = "browser-prompt-stop-switch",
                label = env.I18n:T("prompt.stop"),
                checked = mine and state.status == "shown" and state.stop,
                disabled = not mine,
                onChange = view:Handler(function(event)
                    env.dispatch({ type = "stop", value = event.checked == true })
                end),
            }),
            ui.text(env.I18n:T("prompt.stop"), { size = 13, lineHeight = 20, color = "text-weak", wrap = true, shrink = true, minW = 0 }),
        }))
    end
    local buttons: { ui.Node } = {}
    if dialog.kind ~= "alert" then
        table.insert(buttons, ui.button({
            key = "browser-prompt-cancel",
            text = env.I18n:T(if leave then "prompt.leave.stay" else "prompt.cancel"),
            variant = "neutral",
            disabled = not armed,
            onPress = view:Handler(function()
                answer(env, tab, dialog, false)
            end),
        }))
    end
    table.insert(buttons, ui.button({
        key = "browser-prompt-ok",
        text = env.I18n:T(if leave then "prompt.leave.leave" else "prompt.ok"),
        variant = "contrast",
        disabled = not armed,
        onPress = view:Handler(function()
            answer(env, tab, dialog, true)
        end),
    }))
    table.insert(body, ui.row({ w = "full", gap = 8, justify = "end", mt = 4 }, buttons))
    -- Inside the page's box only, over a dimmed page, as Desktop's dialog scoped to the pane.
    return ui.col({ key = "browser-prompt", pin = { left = 0, right = 0, top = 0, bottom = 0 }, align = "center", justify = "center", pad = 16 }, {
        ui.col({ pin = { left = 0, right = 0, top = 0, bottom = 0 }, fill = "bg-base", opacity = 0.6 }),
        ui.col({
            key = "browser-prompt-card",
            role = "alertdialog",
            label = title,
            w = "full",
            maxW = 400,
            pad = 16,
            gap = 8,
            radius = 12,
            fill = "surface-1",
            shadow = "overlay",
        }, body),
    })
end

return prompt
