--!strict
-- Goal: the session's /goal as a pill above its composer, with Pause, Resume and Clear in the
-- popover it opens. Shows server requests in tasks, server events as signals, a plugin signal as
-- a memo version, and a composer pill whose popover the host owns.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")

type Goal = {
    status: "active" | "paused" | "complete",
    objective: string,
}

type Action = "pause" | "resume" | "clear"

local function parseGoal(value: any): Goal?
    if type(value) ~= "table" or type(value.objective) ~= "string" then
        return nil
    end
    local status = value.status
    if status == "active" or status == "paused" or status == "complete" then
        return { status = status, objective = value.objective }
    end
    return nil
end

return plugin.defineWindow(function(ctx)
    local Sessions = ctx:GetService("Sessions")
    local Server = ctx:GetService("Server")
    local Toast = ctx:GetService("Toast")
    local Popover = ctx:GetService("Popover")

    local goals: { [string]: Goal? } = {}
    local busy: { [string]: boolean? } = {}
    -- Fired on every real change: it redraws the window, and its version keys the menu's memo.
    local changed = ctx:CreateSignal()

    local function load(sessionId: string, directory: string)
        task.spawn(function()
            local path = `/api/rpc/opencode.goal/get?location%5Bdirectory%5D={Server:EncodeUri(directory)}`
            local res = Server:RequestAsync("POST", path, { input = { sessionID = sessionId } })
            if res.ok and res.json then
                local output = res.json.output or (res.json.data and res.json.data.output)
                goals[sessionId] = parseGoal(output)
                changed:Fire()
            end
        end)
    end

    local function run(sessionId: string, action: Action)
        Popover:Close()
        busy[sessionId] = true
        changed:Fire()
        task.spawn(function()
            local res =
                Server:RequestAsync("POST", `/api/session/{sessionId}/command`, { name = "goal", text = action })
            busy[sessionId] = nil
            if not res.ok then
                Toast:Error("Couldn't update the goal", res.error)
            end
            changed:Fire()
        end)
    end

    Server:OnEvent("rpc.opencode.goal.changed"):Connect(function(event)
        local data = if event.data.sessionID then event.data else event.data.data
        if data and type(data.sessionID) == "string" then
            goals[data.sessionID] = parseGoal(data.goal)
            changed:Fire()
        end
    end)

    Sessions.RoutedChanged:Connect(function(session)
        if session and goals[session.id] == nil then
            load(session.id, session.directory)
        end
    end)

    local routed = Sessions:Routed()
    if routed then
        load(routed.id, routed.directory)
    end

    ctx:AddComposerPill({
        id = "goal",
        show = function(props)
            local goal = goals[props.sessionId]
            if not goal then
                return nil
            end
            if goal.status == "paused" then
                return { label = "Goal paused", icon = "circle-exclamation", tone = "warning", title = goal.objective }
            elseif goal.status == "complete" then
                return { label = "Goal met", icon = "circle-check", tone = "success", title = goal.objective }
            end
            return { label = "Goal", icon = "outline-steer", title = goal.objective }
        end,
        renderMenu = function(view, props)
            local sid = props.sessionId
            return view:Memo("goal-menu", `{sid}:{changed:Version()}`, function()
                local goal = goals[sid]
                local disabled = busy[sid] == true
                local actions: { ui.Node } = { ui.spacer() }
                if goal and goal.status == "active" then
                    table.insert(
                        actions,
                        ui.button({
                            key = "goal-pause",
                            text = "Pause",
                            icon = "stop",
                            variant = "ghost",
                            size = "small",
                            disabled = disabled,
                            onPress = view:Handler(function()
                                run(sid, "pause")
                            end),
                        })
                    )
                elseif goal and goal.status == "paused" then
                    table.insert(
                        actions,
                        ui.button({
                            key = "goal-resume",
                            text = "Resume",
                            icon = "refresh",
                            variant = "ghost",
                            size = "small",
                            disabled = disabled,
                            onPress = view:Handler(function()
                                run(sid, "resume")
                            end),
                        })
                    )
                end
                table.insert(
                    actions,
                    ui.button({
                        key = "goal-clear",
                        text = "Clear",
                        icon = "outline-trash",
                        variant = "ghost",
                        size = "small",
                        disabled = disabled,
                        onPress = view:Handler(function()
                            run(sid, "clear")
                        end),
                    })
                )
                return ui.row({ key = "goal-actions", w = "full", gap = 4, px = 4, pt = 4 }, actions)
            end)
        end,
    })

    ctx:AddCommand({
        id = "clear",
        title = "Goal: Clear",
        section = "session",
        slash = { name = "goal-clear" },
        requiresSession = true,
        run = function()
            local session = Sessions:Routed()
            if not session then
                return false
            end
            run(session.id, "clear")
        end,
    })
end)
