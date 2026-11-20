--!strict
-- Desktop's built-in Goal plugin (`packages/gui-plugins/src/goal`): the session's `/goal` as a
-- pill above its composer with its own glyph, Following goal, Goal paused or Goal reached, and a
-- popover with the objective, the last reason and the actions that apply. The server's goal
-- plugin owns the goal and serves it over its RPC (`opencode.goal`): `get` reads it, `changed`
-- keeps it current, and the actions run `/goal pause|resume|clear` as the session's command.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local machine = require("@self/machine")

-- The pill and the popover's status line of each goal status (`renderer.tsx`). The glyphs are the
-- plugin's own `icons/*.svg`, tinted by `tone`.
local VIEWS: { [machine.Status]: { label: string, status: string, icon: string, tone: Tone, color: Color } } = {
    active = { label = "active", status = "statusActive", icon = "active", tone = "accent", color = "text-interactive" },
    paused = { label = "paused", status = "statusPaused", icon = "paused", tone = "warning", color = "status-warning" },
    complete = { label = "reached", status = "statusFinished", icon = "complete", tone = "success", color = "status-success" },
}

-- `goal.css`: the objective and reason in 13 pt body text, the status line in 12 pt.
local BODY = table.freeze({ size = 13, lineHeight = 20, tracking = -0.04, color = "text-chrome", dir = "auto", wrap = true } :: UiTextProps)
local STATUS = table.freeze({ size = 12, lineHeight = 18, tracking = 0.05 } :: UiTextProps)
-- The host popover is 320 wide inside a 0.5 hairline, with a 4 pt inset; the panel pads 8 more.
local POPOVER_INNER = 319
local INSET = 12

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
    -- a visual change. A new objective, status, reason or set of enabled menu actions is.
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
        local moved = goal and oldGoal and (goal.status ~= oldGoal.status or goal.objective ~= oldGoal.objective or goal.reason ~= oldGoal.reason)
        if key ~= previous or oldBusy ~= busy or moved then
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
            return { label = I18n:T(view.label), icon = view.icon, tone = view.tone }
        end,
        -- The popover is all the plugin's (`renderer.tsx`, `goal.css`): the objective, then,
        -- once the goal has a reason, a hairline and the colored status over the reason, then
        -- Pause or Resume beside a red Clear, or only Dismiss once the goal is reached, each
        -- waiting while one runs.
        renderMenu = function(view, props)
            local sid = props.sessionId
            return view:Memo(`goal-menu-{sid}`, `{changed:Version()}:{I18n:Locale().id}`, function()
                local goal = machine.goal(state, sid)
                if goal == nil then
                    return nil
                end
                local busy = machine.busy(state, sid)
                local function button(action: machine.Action, labelKey: string, variant: ButtonVariant?): ui.Node
                    return ui.button({
                        key = `goal-{labelKey}`,
                        text = I18n:T(labelKey),
                        variant = variant,
                        size = "small",
                        disabled = busy,
                        onPress = view:Handler(function()
                            Popover:Close()
                            dispatch({ type = "command", key = sid, action = action })
                        end),
                    })
                end
                local actions: { ui.Node } = {}
                if goal.status == "active" then
                    table.insert(actions, button("pause", "pause"))
                elseif goal.status == "paused" then
                    table.insert(actions, button("resume", "resume"))
                end
                -- A reached goal has nothing left to stop, so clearing it only dismisses the pill.
                if goal.status == "complete" then
                    table.insert(actions, button("clear", "dismiss"))
                else
                    table.insert(actions, button("clear", "clear", "danger"))
                end
                local children: { ui.Node } = {
                    ui.col({ key = "goal-objective", w = "full", maxH = 160, scrollY = true }, {
                        ui.text(goal.objective, BODY, { selectable = true }),
                    }),
                }
                -- Until the goal has a reason, the status is the pill's alone.
                local reason = goal.reason
                if reason then
                    local look = VIEWS[goal.status]
                    table.insert(children, ui.col({ key = "goal-divider", w = POPOVER_INNER, h = 0.5, ml = -INSET, fill = "border-base" }))
                    table.insert(children, ui.col({ key = "goal-why", w = "full" }, {
                        ui.text(I18n:T(look.status), STATUS, { key = "goal-status", color = look.color }),
                        ui.text(reason, BODY, { key = "goal-reason", selectable = true }),
                    }))
                end
                table.insert(children, ui.row({ key = "goal-actions", w = "full", gap = 8, wrap = true, pt = 4 }, actions))
                return ui.col({ key = "goal-panel", w = "full", gap = 8, pt = 4, px = INSET - 4, pb = 8 }, children)
            end)
        end,
    })
end)
