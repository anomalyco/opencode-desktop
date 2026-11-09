--!strict
-- Remote access as Desktop's main process changes it (`main-machine.ts`): the background
-- service's setting, read and written through `opencode service`, one command at a time. A
-- change restarts the service, so its steps run as one job and no read lands between them. A
-- change first reads the setting, so one that already matches restarts nothing, unless the
-- caller asks for the restart a tunnel that never connected needs. `step` changes the state and
-- returns the effects; `call` is the window's request id each job answers.

local service = {}

-- Ceiling for `opencode service get remote`. Every command has one, so a hung command never
-- holds the queue.
service.READ_TIMEOUT = 20
-- `opencode service set remote`, which creates the tunnel the first time it turns on.
service.SET_TIMEOUT = 180
-- `opencode service start`, after a change.
service.START_TIMEOUT = 60

export type Request = { kind: "get-remote" } | { kind: "set-remote", enabled: boolean, restart: boolean }
export type Step = "reading" | "checking" | "setting" | "starting"
export type Active = { attempt: number, call: number, request: Request, step: Step }

-- `open` runs `active` and keeps `pending` in order; `disposed` answers nothing more.
export type State = { status: "open", issued: number, active: Active?, pending: { { call: number, request: Request } } } | { status: "disposed" }

export type Event =
    { type: "requested", call: number, request: Request }
    | { type: "succeeded", attempt: number, stdout: string }
    | { type: "failed", attempt: number, message: string }
    | { type: "disposed" }

export type Effect =
    { type: "exec", attempt: number, args: { string }, timeout: number }
    | { type: "resolve", call: number, value: boolean? }
    | { type: "reject", call: number, message: string }

local STOPPED = "Pairing stopped"

function service.create(): State
    return { status = "open", issued = 0, pending = {} }
end

local function exec(s: any, active: Active, args: { string }, timeout: number, effects: { Effect })
    s.issued += 1
    active.attempt = s.issued
    s.active = active
    table.insert(effects, { type = "exec", attempt = active.attempt, args = args, timeout = timeout })
end

local function setArgs(enabled: boolean): { string }
    return { "set", "remote", if enabled then "true" else "false" }
end

-- Starts the next pending job once none runs.
local function advance(s: any, effects: { Effect }): { Effect }
    local head = s.pending[1]
    if s.active or not head then
        return effects
    end
    table.remove(s.pending, 1)
    local request: any = head.request
    if request.kind == "get-remote" then
        exec(s, { attempt = 0, call = head.call, request = request, step = "reading" }, { "get", "remote" }, service.READ_TIMEOUT, effects)
    elseif request.restart then
        exec(s, { attempt = 0, call = head.call, request = request, step = "setting" }, setArgs(request.enabled), service.SET_TIMEOUT, effects)
    else
        exec(s, { attempt = 0, call = head.call, request = request, step = "checking" }, { "get", "remote" }, service.READ_TIMEOUT, effects)
    end
    return effects
end

function service.step(state: State, e: Event): { Effect }
    if state.status == "disposed" then
        if e.type == "requested" then
            return { { type = "reject", call = e.call, message = STOPPED } }
        end
        return {}
    end
    local s = state :: any
    if e.type == "disposed" then
        local effects: { Effect } = {}
        if s.active then
            table.insert(effects, { type = "reject", call = s.active.call, message = STOPPED })
        end
        for _, item in s.pending do
            table.insert(effects, { type = "reject", call = item.call, message = STOPPED })
        end
        local gone = state :: any
        table.clear(gone)
        gone.status = "disposed"
        return effects
    end
    if e.type == "requested" then
        table.insert(s.pending, { call = e.call, request = e.request })
        return advance(s, {})
    end
    local active: Active? = s.active
    if not active or active.attempt ~= (e :: any).attempt then
        return {}
    end
    s.active = nil
    if e.type == "failed" then
        return advance(s, { { type = "reject", call = active.call, message = e.message } })
    end
    local stdout = (e :: any).stdout :: string
    local request: any = active.request
    if active.step == "reading" then
        return advance(s, { { type = "resolve", call = active.call, value = stdout == "true" } })
    elseif active.step == "checking" then
        if (stdout == "true") == request.enabled then
            return advance(s, { { type = "resolve", call = active.call } })
        end
        local effects: { Effect } = {}
        exec(s, { attempt = 0, call = active.call, request = request, step = "setting" }, setArgs(request.enabled), service.SET_TIMEOUT, effects)
        return effects
    elseif active.step == "setting" then
        -- Changing the setting stops the service; starting it brings it back, with the tunnel
        -- when remote access is on.
        local effects: { Effect } = {}
        exec(s, { attempt = 0, call = active.call, request = request, step = "starting" }, { "start" }, service.START_TIMEOUT, effects)
        return effects
    end
    return advance(s, { { type = "resolve", call = active.call } })
end

return service
