--!strict
-- Desktop's built-in Goal plugin (`packages/gui-plugins/src/goal`): the session's `/goal` as a
-- pill above its composer, Goal, Goal paused or Goal met, and a popover headed by the objective
-- with Pause, Resume and Clear. The server's goal plugin owns the goal and serves it over its RPC
-- (`opencode.goal`): `get` reads it, `changed` keeps it current, and the actions run
-- `/goal pause|resume|clear` as the session's command.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local machine = require("@self/machine")

local VIEWS: { [machine.Status]: { label: string, icon: string, tone: Tone? } } = {
    active = { label = "title", icon = "steer", tone = nil },
    paused = { label = "paused", icon = "circle-exclamation", tone = "warning" },
    complete = { label = "met", icon = "circle-check", tone = "success" },
}

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Sessions = ctx:GetService("Sessions")
    local Server = ctx:GetService("Server")
    local Toast = ctx:GetService("Toast")
    local Popover = ctx:GetService("Popover")
    local state = machine.new()
    -- Fired on every real change: it redraws the window and keys the menu's memo.
    local changed = ctx:CreateSignal()

    local dispatch: (event: machine.Event) -> ()

    -- Runs one request under `attempt` and settles it once: its answer, or its timeout.
    local function guard(
        timeout: number,
        request: () -> ServerResponse,
        done: (res: ServerResponse) -> machine.Event,
        timedOut: machine.Event
    )
        local settled = false
        task.delay(timeout, function()
            if not settled then
                settled = true
                dispatch(timedOut)
            end
        end)
        task.spawn(function()
            -- A window without a server (a fixture run) answers nothing.
            local res = request() or ({ ok = false, status = 0, text = "", error = "" } :: any)
            if not settled then
                settled = true
                dispatch(done(res))
            end
        end)
    end

    local function run(effects: { machine.Effect })
        for _, effect in effects do
            if effect.type == "fetch" then
                local attempt = effect.attempt
                local path = `/api/rpc/opencode.goal/get?location%5Bdirectory%5D={Server:EncodeUri(effect.directory)}`
                local body = { input = { sessionID = effect.session } }
                guard(machine.SNAPSHOT_TIMEOUT, function()
                    return Server:RequestAsync("POST", path, body)
                end, function(res)
                    if not res.ok then
                        return { type = "snapshot.failed", attempt = attempt }
                    end
                    local json = res.json or {}
                    local output = json.output or (if type(json.data) == "table" then json.data.output else nil)
                    return { type = "snapshot.loaded", attempt = attempt, goal = machine.parse(output) }
                end, { type = "snapshot.failed", attempt = attempt })
            elseif effect.type == "schedule-retry" then
                local attempt = effect.attempt
                task.delay(effect.delay, function()
                    dispatch({ type = "retry.due", attempt = attempt })
                end)
            elseif effect.type == "command" then
                local attempt, key = effect.attempt, effect.session
                local body = { name = "goal", text = effect.action }
                guard(machine.COMMAND_TIMEOUT, function()
                    return Server:RequestAsync("POST", `/api/session/{key}/command`, body)
                end, function(res)
                    return {
                        type = "command.settled",
                        key = key,
                        attempt = attempt,
                        error = if res.ok then nil else (res.error or ""),
                    }
                end, { type = "command.timed-out", key = key, attempt = attempt })
            elseif effect.type == "toast-error" then
                Toast:Error(I18n:T("requestFailed"), effect.message)
            end
        end
    end

    -- Only the routed session can have a visible pill. Starting a fetch or a retry is not
    -- a visual change. A new objective, status or set of enabled menu actions is.
    local function shown(): (string?, machine.Goal?, boolean)
        local target = state.target
        if target == nil then return nil, nil, false end
        local goal = machine.goal(state, target.key)
        if goal == nil then return nil, nil, false end
        return target.key, goal, machine.busy(state, target.key)
    end

    dispatch = function(event)
        local previous, oldGoal, oldBusy = shown()
        local effects = machine.step(state, event)
        local key, goal, busy = shown()
        if key ~= previous or oldBusy ~= busy or (goal and oldGoal and (goal.status ~= oldGoal.status or goal.objective ~= oldGoal.objective)) then
            changed:Fire()
        end
        run(effects)
    end

    -- The goal attaches to the routed session and its folder, again when either changes. The
    -- session changes for much else (its title, cost, messages): those leave the goal as it is,
    -- and redraw nothing.
    local function attach(session: Session?)
        if session and session.id ~= "" then
            -- The machine keeps an attach to its own target as it is (`machine.step`).
            local t = state.target
            if t and t.key == session.id and t.directory == session.directory then
                return
            end
            dispatch({ type = "attach", key = session.id, directory = session.directory })
        end
    end
    Sessions.RoutedChanged:Connect(attach)
    Sessions.Changed:Connect(function(session)
        local routed = Sessions:Routed()
        if routed and routed.id == session.id then
            attach(session)
        end
    end)
    attach(Sessions:Routed())

    Server:OnEvent("rpc.opencode.goal.changed"):Connect(function(event)
        local payload = event.data
        local data = if type(payload) == "table" and payload.sessionID == nil then payload.data else payload
        if type(data) == "table" and type(data.sessionID) == "string" then
            dispatch({ type = "changed", key = data.sessionID, goal = machine.parse(data.goal) })
        end
    end)

    ctx:AddComposerPill({
        id = "goal",
        show = function(props)
            local goal = machine.goal(state, props.sessionId)
            if goal == nil then
                return nil
            end
            local view = VIEWS[goal.status]
            return { label = I18n:T(view.label), icon = view.icon, tone = view.tone, title = goal.objective }
        end,
        -- The actions under the objective (`flex justify-end gap-1 px-1 pt-1`): Pause while
        -- active, Resume while paused, and Clear, each waiting while one runs.
        renderMenu = function(view, props)
            local sid = props.sessionId
            return view:Memo(`goal-menu-{sid}`, `{changed:Version()}:{I18n:Locale().id}`, function()
                local goal = machine.goal(state, sid)
                local busy = machine.busy(state, sid)
                local function button(action: machine.Action, icon: string): ui.Node
                    return ui.button({
                        key = `goal-{action}`,
                        text = I18n:T(action),
                        icon = icon,
                        variant = "ghost",
                        size = "small",
                        disabled = busy,
                        onPress = view:Handler(function()
                            Popover:Close()
                            dispatch({ type = "command", key = sid, action = action })
                        end),
                    })
                end
                local actions: { ui.Node } = { ui.spacer() }
                if goal and goal.status == "active" then
                    table.insert(actions, button("pause", "stop"))
                elseif goal and goal.status == "paused" then
                    table.insert(actions, button("resume", "refresh"))
                end
                table.insert(actions, button("clear", "outline-trash"))
                return ui.row({ key = "goal-actions", w = "full", gap = 4, px = 4, pt = 4 }, actions)
            end)
        end,
    })
end)
