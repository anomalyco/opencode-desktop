--!strict
-- Capture heap snapshots (`heap-machine.ts`): one capture at a time, each with its attempt; the
-- command is off while one runs. And the bar's FOCUS toggle (`focus-machine.ts`). Each step
-- returns the effects to run.

local machine = {}

export type Heap = { status: "idle", attempt: number } | { status: "capturing", attempt: number }

export type HeapEvent = { type: "requested" } | { type: "settled", attempt: number, path: string?, error: string? }

export type HeapEffect =
    { type: "capture", attempt: number }
    | { type: "toast-loading" }
    | { type: "toast-saved", path: string }
    | { type: "toast-partial", path: string }
    | { type: "toast-failed" }

machine.idle = { status = "idle", attempt = 0 } :: Heap

function machine.heap(state: Heap, event: HeapEvent): (Heap, { HeapEffect })
    if event.type == "requested" then
        if state.status == "capturing" then
            return state, {}
        end
        local attempt = state.attempt + 1
        return { status = "capturing", attempt = attempt }, {
            { type = "toast-loading" },
            { type = "capture", attempt = attempt },
        }
    end
    if state.status ~= "capturing" or state.attempt ~= event.attempt then
        return state, {}
    end
    local idle: Heap = { status = "idle", attempt = state.attempt }
    if event.path == nil then
        return idle, { { type = "toast-failed" } }
    end
    if event.error then
        return idle, { { type = "toast-partial", path = event.path } }
    end
    return idle, { { type = "toast-saved", path = event.path } }
end

-- The bar's FOCUS toggle (`focus-machine.ts`). `changing` waits for the host to apply `target`;
-- `desired` is the user's latest click, so a click while the call runs flips it, and the
-- follow-up call starts once this one settles. `attempt` fences call results.
export type Focus =
    { status: "idle", attempt: number, enabled: boolean }
    | { status: "changing", attempt: number, target: boolean, desired: boolean }
    | { status: "disposed" }

export type FocusEvent =
    { type: "toggle" }
    | { type: "set.succeeded", attempt: number }
    | { type: "set.failed", attempt: number }
    | { type: "disposed" }

export type FocusEffect = { type: "set", attempt: number, enabled: boolean } | { type: "reset" } | { type: "notify-failed" }

machine.focusIdle = { status = "idle", attempt = 0, enabled = false } :: Focus

local function start(attempt: number, enabled: boolean): (Focus, { FocusEffect })
    return { status = "changing", attempt = attempt, target = enabled, desired = enabled },
        { { type = "set", attempt = attempt, enabled = enabled } }
end

function machine.focus(state: Focus, event: FocusEvent): (Focus, { FocusEffect })
    if state.status == "disposed" then
        return state, {}
    end
    if event.type == "toggle" then
        if state.status == "idle" then
            return start(state.attempt + 1, not state.enabled)
        end
        return { status = "changing", attempt = state.attempt, target = state.target, desired = not state.desired }, {}
    end
    if event.type == "disposed" then
        -- The host may still turn it on for a call in flight, so going away always asks to turn it off.
        local on = if state.status == "idle" then state.enabled else state.target
        return { status = "disposed" }, if on then { { type = "reset" } } else {}
    end
    if state.status ~= "changing" or state.attempt ~= event.attempt then
        return state, {}
    end
    if event.type == "set.succeeded" then
        if state.desired ~= state.target then
            return start(state.attempt + 1, state.desired)
        end
        return { status = "idle", attempt = state.attempt, enabled = state.target }, {}
    end
    -- A user who already clicked back has what they asked for.
    return { status = "idle", attempt = state.attempt, enabled = not state.target },
        if state.desired == state.target then { { type = "notify-failed" } } else {}
end

-- `isFocusToggleActive`: the user's latest choice, ahead of the host confirming it.
function machine.focusActive(state: Focus): boolean
    if state.status == "changing" then
        return state.desired
    end
    return state.status == "idle" and state.enabled
end

return machine
