--!strict
-- Desktop's built-in Pairing plugin (`packages/gui-plugins/src/pairing`), the window half
-- (`renderer.tsx`, `page.tsx`): Settings › Pairing (Pair a device, Remote access, Keep screen
-- active) and the Pair device command. Pair a device turns remote access on, waits for this
-- machine's server to advertise its tunnel, then shows a code from it (`POST /api/pair`) as a QR
-- code and a link, replaced as it expires. Remote access and the display setting go through the
-- app half (`app.luau`) and its `pairing` contract; codes come from the local server itself.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local machine = require("@self/machine")
local copy = require("@self/copy")

-- `text-13-regular` as the page's alerts draw it.
local ALERT: UiTextProps = { size = 13, weight = "regular", lineHeight = 20, color = "status-danger", role = "alert" }

-- The origin of the HTTPS address among the server's URLs: it listens over plain HTTP, so that
-- one is its remote access tunnel.
local function remoteAddress(urls: any): string?
    for _, url in (if type(urls) == "table" then urls else {}) do
        local host = type(url) == "string" and string.match(url, "^https://([^/?#]+)")
        if host then
            return `https://{host}`
        end
    end
    return nil
end

local function countdown(seconds: number): string
    return string.format("%d:%02d", seconds // 60, seconds % 60)
end

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Dialog = ctx:GetService("Dialog")
    local Layout = ctx:GetService("Layout")
    local Server = ctx:GetService("Server")
    local System = ctx:GetService("System")
    local Qr = ctx:GetService("Qr")
    local Contracts = ctx:GetService("Contracts")
    local live = Contracts:Use("pairing")
    local rev = ctx:CreateSignal()

    local function api(): any
        if live:State() ~= "active" then
            return nil
        end
        return live:Get()
    end

    -- The page's state, made when the page first draws, as Desktop's page mounts.
    local state: machine.State? = nil
    local copyState = copy.create()
    -- Each attempt's timer, and the app half call it waits on with the event its answer makes.
    local timers: { [number]: thread } = {}
    local calls: { [number]: { call: number, answer: (result: any) -> machine.Event } } = {}
    local copyTimer: thread? = nil
    local ticker: thread? = nil
    local main = ""
    local dispatch: (event: machine.Event) -> ()
    local ensure: () -> ()

    local function cancel(thread: thread?)
        if thread and thread ~= coroutine.running() and coroutine.status(thread) == "suspended" then
            task.cancel(thread)
        end
    end

    local function collect()
        local a = api()
        if not a then
            return
        end
        for attempt, c in calls do
            local result = a.results[tostring(c.call)]
            if result then
                calls[attempt] = nil
                dispatch(c.answer(result))
            end
        end
    end

    -- Asks the app half; `answer` turns its result into the machine's event. A missing app half
    -- answers as a failure at once.
    local function ask(attempt: number, kind: string, payload: any, answer: (result: any) -> machine.Event)
        local a = api()
        if not a then
            return dispatch(answer({ error = I18n:T("common.requestFailed") }))
        end
        calls[attempt] = { call = a.request(kind, payload), answer = answer }
        collect()
    end

    local function timer(attempt: number, seconds: number, event: machine.Event)
        timers[attempt] = task.delay(seconds, function()
            timers[attempt] = nil
            calls[attempt] = nil
            dispatch(event)
        end)
    end

    -- The host's answer when the app has no server of its own: pairing can't work at all, so the
    -- dialog says so instead of polling for a tunnel.
    local NO_LOCAL_SERVER = "This app has no local server"

    local function serverRequest(method: HttpMethod, path: string): (any?, boolean)
        local answer = Server:RequestAsync(method, path)
        if answer and not answer.ok and answer.error == NO_LOCAL_SERVER then
            return nil, true
        end
        if not answer or not answer.ok or type(answer.json) ~= "table" then
            return nil, false
        end
        local json = answer.json
        return if type(json.data) == "table" then json.data else json
    end

    local function run(effect: machine.Effect)
        local t = effect.type
        local attempt = effect.attempt
        if t == "abort" then
            cancel(timers[attempt])
            timers[attempt] = nil
            calls[attempt] = nil
        elseif t == "open-dialog" then
            Dialog:Open("pair")
        elseif t == "wait" then
            timer(attempt, effect.delay, { type = "wait.ended", kind = effect.kind, attempt = attempt })
        elseif t == "read-remote" then
            timer(attempt, effect.timeout, { type = "read.done", attempt = attempt })
            ask(attempt, "remote", nil, function(r)
                return { type = "read.done", attempt = attempt, enabled = if r.error then nil else r.value == true }
            end)
        elseif t == "set-remote" then
            timer(attempt, effect.timeout, { type = "change.done", attempt = attempt, ok = false })
            ask(attempt, "setRemote", { enabled = effect.enabled, restart = effect.restart }, function(r)
                return { type = "change.done", attempt = attempt, ok = r.error == nil }
            end)
        elseif t == "read-screen" then
            local a = api()
            dispatch({ type = "screen.read.done", attempt = attempt, enabled = if a then a.screenActive == true else nil })
        elseif t == "set-screen" then
            timer(attempt, effect.timeout, { type = "screen.change.done", attempt = attempt, ok = false })
            ask(attempt, "setScreen", { enabled = effect.enabled }, function(r)
                return { type = "screen.change.done", attempt = attempt, ok = r.error == nil }
            end)
        elseif t == "poll-tunnel" then
            timer(attempt, effect.timeout, { type = "tunnel.missing", attempt = attempt })
            task.spawn(function()
                local info, noServer = serverRequest("GET", "local:/api/info")
                local tunnel = info and remoteAddress(info.urls)
                if timers[attempt] and noServer then
                    dispatch({ type = "server.changed", available = false })
                elseif timers[attempt] then
                    dispatch(if tunnel then { type = "tunnel.found", attempt = attempt, tunnel = tunnel } else { type = "tunnel.missing", attempt = attempt })
                end
            end)
        elseif t == "fetch-code" then
            timer(attempt, effect.timeout, { type = "code.failed", attempt = attempt })
            task.spawn(function()
                local pairing, noServer = serverRequest("POST", "local:/api/pair")
                if not timers[attempt] then
                    return
                end
                if noServer then
                    return dispatch({ type = "server.changed", available = false })
                end
                if pairing and type(pairing.code) == "string" then
                    dispatch({ type = "code.loaded", attempt = attempt, code = pairing.code, expiresIn = if type(pairing.expires_in) == "number" then pairing.expires_in else 300, now = System:Now() })
                else
                    dispatch({ type = "code.failed", attempt = attempt })
                end
            end)
        end
    end

    -- The countdown is the dialog's only clock: it ticks while a code shows.
    local function clock()
        local ready = state ~= nil and state.dialog.status == "ready"
        if ready and not ticker then
            ticker = task.spawn(function()
                while true do
                    task.wait(1)
                    if ticker ~= coroutine.running() then
                        return
                    end
                    dispatch({ type = "tick", now = System:Now() })
                end
            end)
        elseif not ready and ticker then
            cancel(ticker)
            ticker = nil
        end
    end

    local queue: { machine.Event } = {}
    local draining = false
    dispatch = function(event: machine.Event)
        -- A press can land before the deferred first draw made the state.
        if not state then
            ensure()
        end
        table.insert(queue, event)
        if draining then
            return
        end
        draining = true
        while #queue > 0 do
            for _, effect in machine.step(state :: machine.State, table.remove(queue, 1) :: machine.Event) do
                run(effect)
            end
        end
        draining = false
        clock()
        rev:Fire()
    end

    local function dispatchCopy(event: copy.Event)
        local nextState, effects = copy.transition(copyState, event)
        copyState = nextState
        for _, effect in effects do
            if effect.type == "cancel-timer" then
                cancel(copyTimer)
                copyTimer = nil
            elseif effect.type == "schedule-feedback" then
                local attempt = effect.attempt
                copyTimer = task.delay(effect.delay, function()
                    copyTimer = nil
                    dispatchCopy({ type = "feedback.ended", attempt = attempt })
                end)
            elseif effect.type == "write" then
                local attempt, url = effect.attempt, effect.url
                task.spawn(function()
                    local ok = pcall(function()
                        System:CopyText(url)
                    end)
                    dispatchCopy({ type = if ok then "copy.succeeded" else "copy.failed", attempt = attempt } :: any)
                end)
            end
        end
        rev:Fire()
    end

    -- Follows the app half through its generations, and its pending and inactive states.
    local function follow()
        local status = live:State()
        local key = if status == "active" then `active:{live:Generation()}` else status
        if key == main or not state then
            return
        end
        -- A new value from the same app half (an answer landing) is no new generation.
        if status == "active" and string.sub(main, 1, 7) == "active:" then
            return
        end
        main = key
        dispatch({ type = "main.changed", active = status == "active", pending = status == "pending" })
    end

    ensure = function()
        if state then
            return
        end
        state = machine.create(true)
        follow()
        rev:Fire()
    end

    live.Changed:Connect(function()
        follow()
        collect()
        rev:Fire()
    end)

    local function version(): string
        return `{live:Generation()}|{rev:Version()}`
    end

    local function pairRow(v: View, disabled: boolean): UiNode
        return ui.settingsRow(
            { id = "pairing", title = I18n:T("pair.title"), description = I18n:T("pair.description") },
            ui.button({
                key = "settings-pair-device",
                text = I18n:T("pair.open"),
                variant = "neutral",
                disabled = disabled,
                onPress = v:Handler(function()
                    dispatch({ type = "dialog.opened" })
                end),
            })
        )
    end

    local function page(v: View): UiNode?
        -- The page's state starts on its first draw, which already shows it as the app half's
        -- first answers leave it: remote access reading, the display setting known.
        local s = state
        if not s then
            task.delay(0, ensure)
            local starting = machine.create(true)
            if live:State() == "active" then
                starting.remote = { status = "loading", attempt = 0, waiting = false, failures = 0 }
                -- The app half answers the display setting at once.
                starting.screen = { status = "ready", enabled = api().screenActive == true }
            end
            s = starting
        end
        local view = machine.page(s :: machine.State)
        local rows = { pairRow(v, view.pairDisabled) }
        if view.mainActive then
            table.insert(rows, ui.settingsRow(
                { id = "settings-remote-access", title = I18n:T("remote.title"), description = I18n:T("remote.description") },
                ui.switch({
                    key = "remote-access-switch",
                    label = I18n:T("remote.title"),
                    checked = view.remoteChecked,
                    disabled = view.remoteDisabled,
                    onChange = v:Handler(function(event)
                        dispatch({ type = "toggled", enabled = (event :: any).checked == true })
                    end),
                })
            ))
            table.insert(rows, ui.settingsRow(
                { id = "settings-keep-screen-active", title = I18n:T("screenActive.title"), description = I18n:T("screenActive.description") },
                ui.switch({
                    key = "keep-screen-active-switch",
                    label = I18n:T("screenActive.title"),
                    checked = view.screenChecked,
                    disabled = view.screenDisabled,
                    onChange = v:Handler(function(event)
                        dispatch({ type = "screen.toggled", enabled = (event :: any).checked == true })
                    end),
                })
            ))
        end
        local section: { UiNode } = { ui.settingsList({ key = "pairing-list" }, rows) }
        if view.remoteError then
            table.insert(section, ui.text(I18n:T("remote.error"), ALERT))
        end
        if view.screenError then
            table.insert(section, ui.row({ gap = 8, align = "center", wrap = true }, {
                ui.text(I18n:T("screenActive.error"), ALERT),
                ui.button({ key = "pairing-screen-retry", text = I18n:T("retry"), variant = "ghost", size = "small", onPress = v:Handler(function()
                    dispatch({ type = "screen.retry" })
                end) }),
            }))
        end
        if view.unavailable then
            table.insert(section, ui.text(I18n:T("pair.unavailable"), ALERT))
        end
        return ui.col({ key = "pairing-section", w = "full", gap = 16 }, section)
    end

    -- The code's frame, which also holds the setup's progress and its failure, so the dialog keeps
    -- its size: the content width, square.
    local function frame(props: { [string]: any }, children: { UiNode }): UiNode
        local p: any = { key = "pairing-qr-frame", w = "full", h = 368, radius = 6, fill = "surface-1", pad = 24, gap = 8, center = true }
        for k, val in props do
            p[k] = val
        end
        return ui.col(p, children)
    end

    local function close()
        Dialog:Close()
        dispatch({ type = "dialog.closed" })
        dispatchCopy({ type = "reset" })
    end

    local function dialogBody(v: View): UiNode?
        local s = state
        if not s then
            return nil
        end
        local d = s.dialog :: any
        local refresh = function(disabled: boolean): UiNode
            return ui.button({ key = "pairing-refresh", text = I18n:T("refresh"), variant = "ghost", disabled = disabled, onPress = v:Handler(function()
                dispatchCopy({ type = "reset" })
                dispatch({ type = "code.refresh" })
            end) })
        end
        local body: { UiNode } = {}
        local url = if d.status == "ready" then d.url else nil
        local copyView = copy.view(copyState, url)
        if d.status == "ready" then
            -- The QR code carries the remote address as `opencode pair --remote` prints it,
            -- {"code","urls"} JSON, which the app's scanner reads. Scanners expect dark modules on
            -- a light quiet zone, so the code keeps black on white in every theme (`page.tsx`).
            local qr = Qr:Encode(`\{"code":"{d.code}","urls":["{d.tunnel}"]\}`)
            table.insert(body, frame({ fill = "bg-base", role = "img", label = I18n:T("qr") }, {
                ui.box({ w = 320, h = 320, fill = ui.hex("#ffffff") }, {
                    ui.icon(qr.icon, { size = 320, color = ui.hex("#000000") }),
                }),
            }))
            table.insert(body, ui.row({ w = "full", gap = 8, align = "center", wrap = true }, {
                -- Desktop marks this countdown role="timer" (page.tsx), but Rune's Luau bridge rejects that
                -- role until https://github.com/anomalyco/rune/pull/1089 is in the pin, and the dialog stopped
                -- drawing as a code arrived. Plain text until then: read, never announced as it ticks.
                ui.text(I18n:T("expires", { time = countdown(machine.remaining(s, System:Now())) }), { size = 12, weight = "regular", lineHeight = 16, tabular = true, color = "text-muted" }),
                ui.spacer(),
                ui.row({ gap = 8, align = "center" }, {
                    refresh(d.refreshing ~= nil),
                    ui.button({ key = "pairing-copy-link", text = I18n:T(copyView.label), icon = copyView.icon, variant = "neutral", disabled = copyView.disabled, onPress = v:Handler(function()
                        dispatchCopy({ type = "copy.requested", url = d.url })
                    end) }),
                }),
            }))
        elseif d.status == "code" or d.status == "turning-on" or d.status == "tunnel" then
            local inner: { UiNode } = { ui.loader({ size = 20 }) }
            if d.status ~= "code" then
                table.insert(inner, ui.text(I18n:T("setup.title"), { size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", textAlign = "center" }))
                table.insert(inner, ui.text(I18n:T("setup.description"), { size = 12, weight = "regular", lineHeight = 16, color = "text-muted", textAlign = "center" }))
            end
            table.insert(body, frame({ role = "status" }, inner))
        elseif d.status == "failed" then
            table.insert(body, frame({ gap = 12 }, {
                ui.text(I18n:T("setup.error"), ALERT),
                ui.button({ key = "pairing-retry", text = I18n:T("retry"), variant = "neutral", onPress = v:Handler(function()
                    dispatch({ type = "retry" })
                end) }),
            }))
        elseif d.status == "code-error" then
            -- A failed code fetch hides the link, so the way to try again stays next to the error.
            table.insert(body, ui.col({ w = "full", gap = 4, align = "center" }, { ui.text(I18n:T("error"), ALERT), refresh(false) }))
        else
            table.insert(body, frame({ gap = 12 }, { ui.text(I18n:T("pair.unavailable"), ALERT) }))
        end
        if copyView.error then
            table.insert(body, ui.text(I18n:T("copy.error"), ALERT))
        end
        return ui.col({ key = "pairing-dialog", w = "full" }, {
            ui.row({ key = "pairing-dialog-header", w = "full", pad = 16 }, {
                ui.col({ grow = 1, shrink = true, minW = 0, gap = 4 }, {
                    ui.text(I18n:T("pair.title"), { size = 15, weight = "medium", lineHeight = 20, tracking = -0.13, color = "text-chrome" }),
                    ui.text(I18n:T("dialog.description"), { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-muted", wrap = true }),
                }),
                ui.box({
                    key = "pairing-dialog-close",
                    role = "button",
                    label = I18n:T("common.close"),
                    w = 20,
                    h = 20,
                    mt = -2,
                    mr = -2,
                    radius = 4,
                    center = true,
                    hover = "surface-hover",
                    onPress = v:Handler(close),
                }, { ui.icon("dialog-close", { size = 16, color = "icon-weak" }) }),
            }),
            ui.col({ key = "pairing-dialog-body", w = "full", gap = 16, px = 16, pb = 16 }, body),
        })
    end

    ctx:AddSettingsPage({
        id = "pairing",
        icon = "pair",
        title = I18n:T("title"),
        description = I18n:T("description"),
        entries = {
            { id = "pairing", title = I18n:T("title"), keywords = "pair device qr phone remote" },
            { id = "settings-remote-access", title = I18n:T("remote.title"), description = I18n:T("remote.description"), keywords = "internet tunnel opentunnel" },
            { id = "settings-keep-screen-active", title = I18n:T("screenActive.title"), description = I18n:T("screenActive.description"), keywords = "display sleep awake local" },
        },
        render = function(v)
            return v:Memo("page", version(), function()
                return page(v)
            end)
        end,
    })

    ctx:AddDialog({
        id = "pair",
        title = I18n:T("pair.title"),
        size = "normal",
        render = function(v)
            return v:Memo("dialog", version(), function()
                return dialogBody(v)
            end)
        end,
        onDismiss = function()
            dispatch({ type = "dialog.closed" })
            dispatchCopy({ type = "reset" })
        end,
    })

    ctx:AddCommand({
        id = "open",
        title = I18n:T("command.title"),
        group = I18n:T("command.category.server"),
        run = function()
            Layout:OpenSettings("pairing")
        end,
    })
end)
