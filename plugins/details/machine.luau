--!strict
-- The details popover's explicit machines: a footer action that reads the server's configuration
-- (`config-action-machine.ts`) and an MCP server's switch (`mcp-machine.ts`, its toggle). Each
-- `step` takes a state and an event and returns the next state and the effects to run; the
-- plugin runs the effects.

local machine = {}

-- How long reading the configuration and opening or copying its file may take.
machine.CONFIG_TIMEOUT = 15
-- How long a copied path keeps its mark.
machine.COPIED_FOR = 2
-- How long a switch may take before it frees up again.
machine.TOGGLE_TIMEOUT = 15

export type ConfigState =
    { status: "idle", issued: number }
    | { status: "opening", attempt: number }
    -- A remote server's configuration path is on the clipboard.
    | { status: "copied", attempt: number }

export type ConfigEvent =
    { type: "activate" }
    | { type: "opened", attempt: number, close: boolean }
    | { type: "copied", attempt: number }
    | { type: "failed", attempt: number, error: string }
    | { type: "timed-out", attempt: number }
    | { type: "copied.expired", attempt: number }
    | { type: "hidden" }

export type ConfigEffect =
    { type: "run", attempt: number }
    | { type: "schedule-reset", attempt: number }
    | { type: "close-owner" }
    | { type: "toast-failed", error: string }
    | { type: "toast-timed-out" }

machine.configIdle = { status = "idle", issued = 0 } :: ConfigState

function machine.config(state: ConfigState, event: ConfigEvent): (ConfigState, { ConfigEffect })
    if event.type == "hidden" then
        if state.status ~= "copied" then
            return state, {}
        end
        return { status = "idle", issued = state.attempt }, {}
    end
    if event.type == "activate" then
        if state.status == "opening" then
            return state, {}
        end
        local attempt = (if state.status == "idle" then state.issued else state.attempt) + 1
        return { status = "opening", attempt = attempt }, { { type = "run", attempt = attempt } }
    end
    if event.type == "copied.expired" then
        if state.status ~= "copied" or state.attempt ~= event.attempt then
            return state, {}
        end
        return { status = "idle", issued = state.attempt }, {}
    end
    if state.status ~= "opening" or state.attempt ~= event.attempt then
        return state, {}
    end
    local idle: ConfigState = { status = "idle", issued = state.attempt }
    if event.type == "opened" then
        return idle, if event.close then { { type = "close-owner" } } else {}
    elseif event.type == "copied" then
        return { status = "copied", attempt = state.attempt }, { { type = "schedule-reset", attempt = state.attempt } }
    elseif event.type == "failed" then
        return idle, { { type = "toast-failed", error = event.error } }
    end
    return idle, { { type = "toast-timed-out" } }
end

-- An MCP server's switch: idle, or a connect or disconnect on its way.
export type ToggleState = { status: "idle", issued: number } | { status: "toggling", attempt: number }

export type ToggleEvent =
    { type: "requested", writable: boolean }
    | { type: "settled", attempt: number, error: string? }
    | { type: "timed-out", attempt: number }

export type ToggleEffect =
    { type: "run", attempt: number }
    | { type: "refresh" }
    | { type: "toast-failed", error: string }
    | { type: "toast-read-only" }

machine.toggleIdle = { status = "idle", issued = 0 } :: ToggleState

function machine.toggle(state: ToggleState, event: ToggleEvent): (ToggleState, { ToggleEffect })
    if event.type == "requested" then
        if state.status == "toggling" then
            return state, {}
        end
        if not event.writable then
            return state, { { type = "toast-read-only" } }
        end
        local attempt = state.issued + 1
        return { status = "toggling", attempt = attempt }, { { type = "run", attempt = attempt } }
    end
    if state.status ~= "toggling" or state.attempt ~= event.attempt then
        return state, {}
    end
    local idle: ToggleState = { status = "idle", issued = state.attempt }
    if event.type == "timed-out" then
        return idle, { { type = "refresh" } }
    end
    if event.error then
        return idle, { { type = "toast-failed", error = event.error }, { type = "refresh" } }
    end
    return idle, { { type = "refresh" } }
end

return machine
