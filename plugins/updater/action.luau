--!strict
-- The one update action the settings row offers for a state (`action.ts`).
-- The window's own check or install shows as running at once, before the host reports it, so a
-- second click has nothing to start.

local machine = require("./machine")

export type Action = { label: string, description: string?, run: machine.Action? }

local function main(state: UpdaterState?): Action
    if not state then
        return { label = "action.checkNow" }
    end
    local phase = state.phase
    if phase == "checking" then
        return { label = "action.checking" }
    elseif phase == "downloading" then
        return { label = "action.downloading" }
    elseif phase == "ready" then
        return { label = "action.installRestart", run = "install" }
    elseif phase == "download-required" then
        return { label = "action.download", run = "install" }
    elseif phase == "installing" then
        return { label = "action.installing" }
    elseif phase == "disabled" then
        -- The host says `isolated` when it keeps updates off in a scratch-root run.
        return { label = "action.unavailable", description = if state.error == "isolated" then "check.isolated" else "check.unavailable" }
    end
    return { label = "action.checkNow", run = "check" }
end

return function(state: UpdaterState?, window: machine.State?): Action
    local action = main(state)
    if window and window.status == "checking" and action.run == "check" then
        return { label = "action.checking" }
    end
    if window and window.status == "installing" and action.run == "install" then
        return { label = "action.installing" }
    end
    return action
end
