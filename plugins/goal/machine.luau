--!strict
-- What the window knows of each session's goal (`machine.ts`): only the routed session is
-- fetched; another keeps the goal it last showed (`stale`), hidden, so switching back shows it
-- while the refetch runs. `step` takes an event and returns the effects to run.

local machine = {}

-- How long `get` may take before the snapshot counts as failed.
machine.SNAPSHOT_TIMEOUT = 15
-- How long a pause, resume or clear may take before its button frees up again.
machine.COMMAND_TIMEOUT = 15
-- Waits before the 1st, 2nd and 3rd refetch of a snapshot that failed; then the session shows no
-- goal.
local RETRY_DELAYS = { 1, 2, 4 }

export type Status = "active" | "paused" | "complete"
-- `reason`: why the goal last changed, the reviewer's reason or what paused it.
export type Goal = { objective: string, status: Status, reason: string? }
export type Action = "pause" | "resume" | "clear"

-- One session's goal, as far as this window knows it.
export type Entry =
    -- Fetching the snapshot; `cached` shows until the answer lands.
    { kind: "loading", attempt: number, retries: number, cached: Goal? }
    -- The last fetch failed; refetches when the timer of `attempt` fires. Shows no goal.
    | { kind: "retrying", attempt: number, retries: number }
    -- The snapshot or a later change: authoritative while attached.
    | { kind: "live", goal: Goal? }
    -- Not routed: kept to show while revalidating, never alone.
    | { kind: "stale", goal: Goal }
    -- Every fetch failed (a server without the goal plugin). A change still brings it live.
    | { kind: "failed" }

export type Target = { key: string, directory: string }

export type State = {
    issued: number,
    target: Target?,
    sessions: { [string]: Entry },
    -- The one pause, resume or clear in flight per session.
    commands: { [string]: { attempt: number, action: Action } },
}

export type Event =
    { type: "attach", key: string, directory: string }
    | { type: "changed", key: string, goal: Goal? }
    | { type: "snapshot.loaded", attempt: number, goal: Goal? }
    | { type: "snapshot.failed", attempt: number }
    | { type: "retry.due", attempt: number }
    | { type: "command", key: string, action: Action }
    | { type: "command.settled", key: string, attempt: number, error: string? }
    | { type: "command.timed-out", key: string, attempt: number }

export type Effect =
    { type: "fetch", attempt: number, session: string, directory: string }
    | { type: "schedule-retry", attempt: number, delay: number }
    | { type: "command", attempt: number, session: string, action: Action }
    | { type: "toast-error", message: string? }

function machine.new(): State
    return { issued = 0, target = nil, sessions = {}, commands = {} }
end

-- `Goal.State` as the server sends it; nil for `null` or anything unreadable.
function machine.parse(v: any): Goal?
    if type(v) ~= "table" then
        return nil
    end
    local status = v.status
    if status ~= "active" and status ~= "paused" and status ~= "complete" then
        return nil
    end
    return {
        objective = if type(v.objective) == "string" then v.objective else "",
        status = status,
        reason = if type(v.reason) == "string" and v.reason ~= "" then v.reason else nil,
    }
end

-- The goal the pill shows: a live answer, or the cached one while it revalidates.
function machine.goal(state: State, key: string): Goal?
    local e = state.sessions[key]
    if e == nil then
        return nil
    elseif e.kind == "live" then
        return e.goal
    elseif e.kind == "loading" then
        return e.cached
    end
    return nil
end

function machine.busy(state: State, key: string): boolean
    return state.commands[key] ~= nil
end

local function next(state: State): number
    state.issued += 1
    return state.issued
end

-- Leaving the routed session keeps only a goal it showed, as `stale`.
local function detach(state: State)
    local target = state.target
    if target == nil then
        return
    end
    state.target = nil
    local shown = machine.goal(state, target.key)
    state.sessions[target.key] = (if shown then { kind = "stale", goal = shown } else nil) :: any
end

-- The routed session's entry while its snapshot `attempt` is in flight.
local function loading(state: State, attempt: number): (string?, number)
    local target = state.target
    if target == nil then
        return nil, 0
    end
    local e = state.sessions[target.key]
    if e and e.kind == "loading" and e.attempt == attempt then
        return target.key, e.retries
    end
    return nil, 0
end

function machine.step(state: State, event: Event): { Effect }
    if event.type == "attach" then
        local t = state.target
        if t and t.key == event.key and t.directory == event.directory then
            return {}
        end
        detach(state)
        local old = state.sessions[event.key]
        local cached = if old and old.kind == "stale" then old.goal else nil
        local attempt = next(state)
        state.sessions[event.key] = { kind = "loading", attempt = attempt, retries = 0, cached = cached }
        state.target = { key = event.key, directory = event.directory }
        return { { type = "fetch", attempt = attempt, session = event.key, directory = event.directory } }
    elseif event.type == "changed" then
        -- A change is newer than any snapshot in flight. Another session's change updates only a
        -- goal this window showed.
        local target = state.target
        if target == nil then
            return {}
        end
        if event.key == target.key then
            state.sessions[event.key] = { kind = "live", goal = event.goal }
        elseif state.sessions[event.key] ~= nil then
            state.sessions[event.key] = (if event.goal then { kind = "stale", goal = event.goal } else nil) :: any
        end
        return {}
    elseif event.type == "snapshot.loaded" then
        local key = loading(state, event.attempt)
        if key then
            state.sessions[key] = { kind = "live", goal = event.goal }
        end
        return {}
    elseif event.type == "snapshot.failed" then
        -- A failed refetch drops the cached goal at once: it can no longer be trusted as current.
        local key, retries = loading(state, event.attempt)
        if key == nil then
            return {}
        end
        local delay = RETRY_DELAYS[retries + 1]
        if delay == nil then
            state.sessions[key] = { kind = "failed" }
            return {}
        end
        local attempt = next(state)
        state.sessions[key] = { kind = "retrying", attempt = attempt, retries = retries + 1 }
        return { { type = "schedule-retry", attempt = attempt, delay = delay } }
    elseif event.type == "retry.due" then
        local target = state.target
        local e = if target then state.sessions[target.key] else nil
        if target == nil or e == nil or e.kind ~= "retrying" or e.attempt ~= event.attempt then
            return {}
        end
        state.sessions[target.key] = { kind = "loading", attempt = e.attempt, retries = e.retries, cached = nil }
        return { { type = "fetch", attempt = e.attempt, session = target.key, directory = target.directory } }
    elseif event.type == "command" then
        -- A second click while the first is in flight would fail on the server.
        if state.commands[event.key] then
            return {}
        end
        local attempt = next(state)
        state.commands[event.key] = { attempt = attempt, action = event.action }
        return { { type = "command", attempt = attempt, session = event.key, action = event.action } }
    end
    -- A command settled, with the server's message when it failed, or timed out.
    local c = state.commands[event.key]
    if c == nil or c.attempt ~= event.attempt then
        return {}
    end
    state.commands[event.key] = nil
    if event.type == "command.timed-out" then
        return { { type = "toast-error", message = nil } }
    end
    if event.error then
        return { { type = "toast-error", message = event.error } }
    end
    return {}
end

return machine
