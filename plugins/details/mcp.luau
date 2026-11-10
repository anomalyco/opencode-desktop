--!strict
-- A location's MCP servers as Session details drives them (`server-panel.tsx`'s MCP controller):
-- connects, disconnects and sign-ins through `machine.mcp`, the error dialog the MCP rows open
-- (`McpErrorDialog`), and the sign-in dialog that also takes the redirect URL pasted from the
-- browser (`McpSignInDialog`, driven by `signin`).

local ui = require("@opencode/ui")
local machine = require("./machine")
local signin = require("./signin")

local mcp = {}

-- The sign-in on show: its server, location and attempt, and its machine and timers.
export type SignIn = {
    name: string,
    directory: string,
    integrationID: string,
    attemptID: string,
    state: signin.State,
    deadline: thread?,
    poll: thread?,
    submit: thread?,
    sync: thread?,
}

export type Env = {
    Server: ServerService,
    System: SystemService,
    Dialog: DialogService,
    I18n: I18nService,
    Toast: ToastService,
    editor: Editor,
    -- Each location's MCP state, by directory.
    states: { [string]: machine.McpState },
    signIn: { current: SignIn? },
    -- `?location[directory]=…` for a directory.
    location: (directory: string) -> string,
    -- A list of servers from `/api/mcp`'s answer.
    servers: (json: any) -> { { name: string, status: string?, error: string?, integrationID: string? } },
    redraw: () -> (),
}

local function find(list: { any }, name: string): any
    for _, item in list do
        if item.name == name then
            return item
        end
    end
    return nil
end

function mcp.state(env: Env, directory: string): machine.McpState
    local state = env.states[directory]
    if state == nil then
        state = machine.mcpInitial()
        env.states[directory] = state
    end
    return state
end

local runToggle: (env: Env, directory: string, effect: any) -> ()
local startSignIn: (env: Env, directory: string, name: string, s: machine.McpSignIn) -> ()

-- Steps `directory`'s MCP machine with `event` and runs what it asks for.
function mcp.dispatch(env: Env, directory: string, event: machine.McpEvent)
    local state, effects = machine.mcp(mcp.state(env, directory), event)
    env.states[directory] = state
    for _, effect in effects do
        if effect.type == "run-toggle" then
            runToggle(env, directory, effect)
        elseif effect.type == "open-external" then
            env.System:OpenExternal(effect.url)
        elseif effect.type == "open-sign-in" then
            startSignIn(env, directory, effect.name, effect.signIn)
        elseif effect.type == "open-dialog" then
            -- One dialog at a time: a sign-in on show ends as its dialog closes.
            if env.signIn.current then
                mcp.signInDismissed(env)
            end
            env.Dialog:Open("mcp-error", { attempt = effect.attempt, name = effect.name, directory = directory })
        elseif effect.type == "close-dialog" then
            if env.Dialog:IsOpen("mcp-error") then
                env.Dialog:Close()
            end
        elseif effect.type == "toast-failed" then
            env.Toast:Error(env.I18n:T("requestFailed"), effect.message)
        elseif effect.type == "toast-timed-out" then
            env.Toast:Error(env.I18n:T("requestFailed"), env.I18n:T("mcp.timedOut", { name = effect.name }))
        end
    end
    env.redraw()
end

-- `run-toggle`: connects or disconnects the server, or starts its browser sign-in when it needs
-- one, then reads the list again, all within the attempt's timeout.
runToggle = function(env: Env, directory: string, effect: any)
    local name, attempt, enabled = effect.name, effect.attempt, effect.enabled
    local loc = env.location(directory)
    task.delay(effect.timeout, function()
        mcp.dispatch(env, directory, { type = "toggle.timed-out", name = name, attempt = attempt })
    end)
    task.spawn(function()
        local function failed(error: string?)
            mcp.dispatch(
                env,
                directory,
                { type = "toggle.failed", name = name, attempt = attempt, error = error or "", server = false }
            )
        end
        if not env.Server:CanWrite() then
            return failed(env.I18n:T("readOnly"))
        end
        local res = env.Server:RequestAsync("GET", "/api/mcp" .. loc)
        if not res.ok then
            return failed(res.error)
        end
        local found = find(env.servers(res.json), name)
        if found == nil then
            return mcp.dispatch(env, directory, { type = "toggle.succeeded", name = name, attempt = attempt })
        end
        local verb = if not enabled then "disconnect" elseif found.status ~= "needs_auth" then "connect" else nil
        if verb then
            local done = env.Server:RequestAsync("POST", `/api/experimental/mcp/{env.Server:EncodeUri(name)}/{verb}{loc}`)
            if not done.ok then
                return failed(done.error)
            end
        end
        env.Server:Invalidate("/api/mcp")
        local listed = env.Server:RequestAsync("GET", "/api/mcp" .. loc)
        if not listed.ok then
            return failed(listed.error)
        end
        local current = find(env.servers(listed.json), name)
        local integration = if current then current.integrationID else nil
        if enabled and current and current.status == "needs_auth" and integration then
            local id = env.Server:EncodeUri(integration)
            local info = env.Server:RequestAsync("GET", `/api/integration/{id}{loc}`)
            if not info.ok then
                return failed(info.error)
            end
            local data = if type(info.json) == "table" and type(info.json.data) == "table" then info.json.data else {}
            local method = nil
            for _, m in (if type(data.methods) == "table" then data.methods else {}) do
                if type(m) == "table" and m.type == "oauth" and (type(m.form) ~= "table" or #m.form == 0) then
                    method = m
                    break
                end
            end
            if method == nil then
                return failed(env.I18n:T("mcp.interactiveAuth", { name = name }))
            end
            local started = env.Server:RequestAsync("POST", `/api/integration/{id}/connect/oauth{loc}`, { methodID = method.id })
            if not started.ok then
                return failed(started.error)
            end
            local a = if type(started.json) == "table" and type(started.json.data) == "table"
                then started.json.data
                else started.json or {}
            local time = if type(a.time) == "table" then a.time else {}
            mcp.dispatch(env, directory, {
                type = "toggle.authorizing",
                name = name,
                attempt = attempt,
                url = a.url or "",
                signIn = if a.paste
                    then {
                        integrationID = integration,
                        attemptID = a.attemptID or "",
                        url = a.url or "",
                        lifetime = (time.expires or 0) - (time.created or 0),
                    }
                    else nil,
            })
        end
        -- A successful answer can still leave the connection failed.
        if current and current.status == "failed" then
            return mcp.dispatch(
                env,
                directory,
                { type = "toggle.failed", name = name, attempt = attempt, error = current.error or "", server = true }
            )
        end
        mcp.dispatch(env, directory, { type = "toggle.succeeded", name = name, attempt = attempt })
    end)
end

-- The sign-in's own requests, under `/api/integration/<id>/connect/oauth/<attempt>`.
local function attemptPath(env: Env, s: SignIn, tail: string): string
    local id, attempt = env.Server:EncodeUri(s.integrationID), env.Server:EncodeUri(s.attemptID)
    return `/api/integration/{id}/connect/oauth/{attempt}{tail}{env.location(s.directory)}`
end

local function cancelThread(t: thread?)
    if t then
        task.cancel(t)
    end
end

-- `polled`'s outcome from a status answer (`McpSignInAttemptStatus`).
local function outcomeOf(res: ServerResponse): signin.Outcome
    if not res.ok then
        return { status = "unreachable", message = res.error or "" }
    end
    local d = if type(res.json) == "table" and type(res.json.data) == "table" then res.json.data else res.json or {}
    if d.status == "complete" then
        return { status = "complete" }
    elseif d.status == "expired" then
        return { status = "expired" }
    elseif d.status == "failed" then
        return { status = "failed", message = d.message or "" }
    end
    return { status = "pending" }
end

local runSignIn: (env: Env, s: SignIn, effects: { signin.Effect }) -> ()

-- Steps the sign-in on show with `event` and runs what it asks for.
function mcp.signInStep(env: Env, event: signin.Event)
    local s = env.signIn.current
    if s == nil then
        return
    end
    local state, effects = signin.step(s.state, event)
    s.state = state
    runSignIn(env, s, effects)
    env.redraw()
end

-- `createMcpSignIn`'s driver: the machine decides when each request and timer starts and stops.
runSignIn = function(env: Env, s: SignIn, effects: { signin.Effect })
    for _, effect in effects do
        if effect.type == "deadline.start" then
            cancelThread(s.deadline)
            s.deadline = task.delay(effect.delay / 1000, function()
                mcp.signInStep(env, { type = "waiting.timed-out" })
            end)
        elseif effect.type == "deadline.clear" then
            cancelThread(s.deadline)
            s.deadline = nil
        elseif effect.type == "poll" then
            cancelThread(s.poll)
            local pollAttempt = effect.pollAttempt
            s.poll = task.delay(effect.delay, function()
                local outcome = outcomeOf(env.Server:RequestAsync("GET", attemptPath(env, s, "")))
                mcp.signInStep(env, { type = "polled", pollAttempt = pollAttempt, outcome = outcome })
            end)
        elseif effect.type == "poll.abort" then
            cancelThread(s.poll)
            s.poll = nil
        elseif effect.type == "submit" then
            cancelThread(s.submit)
            local submitAttempt, code = effect.submitAttempt, effect.code
            s.submit = task.spawn(function()
                local res = env.Server:RequestAsync("POST", attemptPath(env, s, "/complete"), { code = code })
                if res.ok then
                    mcp.signInStep(env, { type = "submit.succeeded", submitAttempt = submitAttempt })
                else
                    mcp.signInStep(env, { type = "submit.failed", submitAttempt = submitAttempt, message = res.error or "" })
                end
            end)
        elseif effect.type == "submit.abort" then
            cancelThread(s.submit)
            s.submit = nil
        elseif effect.type == "sync" then
            cancelThread(s.sync)
            local syncAttempt = effect.syncAttempt
            s.sync = task.delay(effect.timeout, function()
                mcp.signInStep(env, { type = "sync.timed-out", syncAttempt = syncAttempt })
            end)
            task.spawn(function()
                env.Server:Invalidate("/api/mcp")
                local res = env.Server:RequestAsync("GET", "/api/mcp" .. env.location(s.directory))
                if res.ok then
                    mcp.signInStep(env, { type = "sync.succeeded", syncAttempt = syncAttempt })
                else
                    mcp.signInStep(env, { type = "sync.failed", syncAttempt = syncAttempt, message = res.error or "" })
                end
            end)
        elseif effect.type == "sync.clear" then
            cancelThread(s.sync)
            s.sync = nil
        elseif effect.type == "refresh" then
            env.Server:Invalidate("/api/mcp")
        elseif effect.type == "open-external" then
            env.System:OpenExternal(effect.url)
        elseif effect.type == "cancel" then
            task.spawn(function()
                env.Server:RequestAsync("DELETE", attemptPath(env, s, ""))
            end)
        elseif effect.type == "close" then
            if env.signIn.current == s then
                env.signIn.current = nil
                env.Dialog:Close()
            end
        end
    end
end

-- The sign-in dialog closed (Esc, outside it, its close button, or another dialog in its place):
-- an attempt still waiting ends.
function mcp.signInDismissed(env: Env)
    mcp.signInStep(env, { type = "disposed" })
    env.signIn.current = nil
end

-- `open-sign-in`: the dialog takes over from any sign-in on show, focuses its field and starts
-- the attempt's deadline and its first status poll (`startMcpSignIn`).
startSignIn = function(env: Env, directory: string, name: string, s: machine.McpSignIn)
    mcp.signInDismissed(env)
    local state, effects = signin.start(s.url, s.lifetime)
    local current: SignIn = {
        name = name,
        directory = directory,
        integrationID = s.integrationID,
        attemptID = s.attemptID,
        state = state,
    }
    env.signIn.current = current
    env.editor:SetText("")
    env.Dialog:Open("mcp-sign-in", {})
    env.editor:Focus()
    runSignIn(env, current, effects)
end

-- Text 13 on 20 at 440, tracking -0.04 (the dialogs' body copy).
local function body(words: string, color: Color?): ui.Node
    return ui.text(
        words,
        { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = color or "text-muted", wrap = true, w = "full" }
    )
end

-- `McpSignInDialog`: what to do, how it stands, Open browser or Retry, and the redirect URL's
-- field with Continue while the server waits.
function mcp.renderSignIn(env: Env, view: View): ui.Node?
    local s = env.signIn.current
    if s == nil then
        return nil
    end
    local I18n = env.I18n
    local v = signin.view(s.state)
    local children: { ui.Node } = { body(I18n:T("mcp.signIn.description", { name = s.name })) }
    local failure = v.failure
    if failure then
        local words = if failure.type == "auth-expired"
            then I18n:T("mcp.signIn.failed", { error = I18n:T("mcp.signIn.expired") })
            elseif failure.type == "auth-failed" then I18n:T("mcp.signIn.failed", { error = failure.message })
            elseif failure.type == "sync-failed" then I18n:T("mcp.signIn.syncFailed", { name = s.name, error = failure.message })
            else I18n:T("mcp.timedOut", { name = s.name })
        table.insert(children, ui.col({ key = "mcp-sign-in-failure", role = "alert", w = "full" }, { body(words, "status-danger") }))
    else
        table.insert(
            children,
            ui.row(
                { key = "mcp-sign-in-waiting", gap = 8, align = "center" },
                { ui.loader({ size = 16 }), ui.text(I18n:T("mcp.signIn.waiting"), {
                    size = 13,
                    weight = "regular",
                    lineHeight = 20,
                    tracking = -0.04,
                    color = "text-muted",
                }) }
            )
        )
    end
    if v.showOpenBrowser or v.canRetry then
        local actions: { ui.Node } = {}
        if v.showOpenBrowser then
            table.insert(actions, ui.button({
                key = "mcp-sign-in-open-browser",
                text = I18n:T("mcp.signIn.openBrowser"),
                icon = "arrow-up-right",
                variant = "neutral",
                onPress = view:Handler(function()
                    mcp.signInStep(env, { type = "browser.open" })
                end),
            }))
        end
        if v.canRetry then
            table.insert(actions, ui.button({
                key = "mcp-sign-in-retry",
                text = I18n:T("tryAgain"),
                variant = "neutral",
                onPress = view:Handler(function()
                    mcp.signInStep(env, { type = "retry.requested" })
                end),
            }))
        end
        table.insert(children, ui.row({ key = "mcp-sign-in-actions", gap = 8, align = "center" }, actions))
    end
    if v.showForm then
        local entry = v.entryError
        local entryText = if entry == nil
            then nil
            elseif entry.type == "rejected" then entry.message
            else I18n:T("mcp.signIn.paste.required")
        local form: { ui.Node } = {
            ui.col({ w = "full", gap = 8 }, {
                ui.text(
                    I18n:T("mcp.signIn.paste.label"),
                    { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text-chrome" }
                ),
                ui.field({
                    key = "mcp-sign-in-field",
                    editor = env.editor,
                    w = "full",
                    placeholder = I18n:T("mcp.signIn.paste.placeholder"),
                    invalid = entryText ~= nil,
                    readOnly = v.locked,
                    label = I18n:T("mcp.signIn.paste.label"),
                }),
            }),
        }
        if entryText then
            table.insert(
                form,
                ui.text(entryText, {
                    key = "mcp-sign-in-error",
                    role = "alert",
                    size = 12,
                    weight = "regular",
                    lineHeight = 16,
                    color = "status-danger",
                    wrap = true,
                    w = "full",
                    mt = -16,
                })
            )
        end
        table.insert(
            form,
            ui.row({}, {
                ui.button({
                    key = "mcp-sign-in-continue",
                    text = I18n:T("continue"),
                    variant = "contrast",
                    disabled = v.locked,
                    onPress = view:Handler(function()
                        mcp.signInStep(env, { type = "paste.submitted" })
                    end),
                }),
            })
        )
        table.insert(children, ui.col({ key = "mcp-sign-in-form", w = "full", gap = 20 }, form))
    end
    -- `Dialog`'s 480 by 368 frame.
    return ui.col({ key = "mcp-sign-in", w = "full", h = 368 }, {
        ui.dialogHeader({ title = I18n:T("mcp.signIn.open", { name = s.name }) }),
        ui.col({ key = "mcp-sign-in-body", w = "full", px = 12, pb = 12, gap = 20 }, children),
    })
end

-- `McpErrorDialog`: the server's name and "Connection failed", the error in a scrolling,
-- selectable block, and Copy details, Disable and Retry under it (`ErrorDetails`).
function mcp.renderError(env: Env, view: View, data: any, status: { status: string?, error: string? }?): ui.Node?
    if type(data) ~= "table" then
        return nil
    end
    local directory, attempt = data.directory :: string, data.attempt :: number
    local current = machine.mcpDialog(mcp.state(env, directory), attempt, if status then status.status else nil)
    if current == nil then
        return nil
    end
    local I18n = env.I18n
    local name, error = current.name, current.error
    local function toggle(enabled: boolean)
        mcp.dispatch(
            env,
            directory,
            { type = "toggle.requested", name = name, enabled = enabled, status = if status then status.status else nil }
        )
    end
    -- `Dialog`'s 480 by 368 frame: the error scrolls within it.
    return ui.col({ key = "session-mcp-error", w = "full", h = 368 }, {
        ui.dialogHeader({ title = name, description = I18n:T("mcp.error.status") }),
        ui.col({ key = "session-mcp-error-body", w = "full", grow = 1, minH = 0 }, {
            ui.col({ key = "error-details", w = "full", grow = 1, minH = 0, gap = 12, px = 16, pb = 16 }, {
                ui.col({
                    key = "error-details-text",
                    w = "full",
                    grow = 1,
                    minH = 0,
                    scrollY = true,
                    py = 8,
                    px = 12,
                    radius = 6,
                    fill = "surface-3",
                }, {
                    ui.text(
                        error,
                        { size = 12, lineHeight = 18, mono = true, color = "text-chrome", wrap = true, selectable = true, w = "full" }
                    ),
                }),
                ui.row({ key = "error-details-actions", w = "full", gap = 8 }, {
                    ui.button({
                        key = "error-details-copy",
                        text = I18n:T(if current.copied then "copied" else "mcp.error.copy"),
                        icon = if current.copied then "check" else "copy",
                        variant = "ghost-muted",
                        onPress = view:Handler(function()
                            env.System:CopyText(I18n:T("mcp.error.details", { name = name, error = error }))
                            mcp.dispatch(env, directory, { type = "error.copied", attempt = attempt, error = error })
                        end),
                    }),
                    ui.spacer(),
                    ui.button({
                        key = "error-details-disable",
                        text = I18n:T("mcp.error.disable"),
                        variant = "ghost",
                        disabled = current.busy,
                        onPress = view:Handler(function()
                            toggle(false)
                        end),
                    }),
                    ui.button({
                        key = "error-details-retry",
                        text = I18n:T("tryAgain"),
                        variant = "neutral",
                        disabled = current.busy,
                        onPress = view:Handler(function()
                            toggle(true)
                        end),
                    }),
                }),
            }),
        }),
    })
end

return mcp
