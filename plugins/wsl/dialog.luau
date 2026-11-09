--!strict
-- The window's side of Add WSL server: the dialog machine (Desktop's `dialog-machine.ts`: which
-- view shows, and the call to the app half it waits for, claimed on the click so the dialog is
-- busy before the app half's state says so) and the probe machine (`probe-machine.ts`: the
-- probes the dialog runs for what it shows, one at a time; a failed one runs again only once the
-- user asks to check again), and the controller that runs their effects as requests to the app
-- half's `wsl` contract.

local model = require("./model")

local dialog = {}

-- Ceiling for the dialog's quick calls, above the app half's 20 s `wsl.exe` timeout.
local ACTION_TIMEOUT = 60
-- Ceiling for the install calls: the app half stops an install after 15 minutes, and a distro
-- install then lists and probes for up to another minute.
local INSTALL_TIMEOUT = 17 * 60
-- Ceiling for one probe call.
local PROBE_TIMEOUT = 60

export type Op = { kind: "install-wsl" } | { kind: "refresh-distros" } | { kind: "install-opencode", distro: string } | { kind: "add-server", distro: string }

-- `main`, `main-running` (with `attempt` and `op`), `catalog`, `catalog-installing` (with
-- `attempt` and `distro`), `completed` (busy while it closes) and `disposed`.
export type DialogState = { status: string, issued: number, selectedDistro: string?, catalogTarget: string?, attempt: number?, op: any, distro: string? }
export type ProbeState = { status: string, issued: number, attempt: number?, command: any, completed: string?, failed: { [string]: boolean } }
type Effect = { type: string, [string]: any }

function dialog.initial(): DialogState
    return { status = "main", issued = 0, selectedDistro = nil }
end

local function run(s: DialogState, op: Op, timeout: number, before: { Effect }): (DialogState, { Effect })
    local attempt = s.issued + 1
    local effects = table.clone(before)
    table.insert(effects, { type = "action.run", attempt = attempt, action = op, timeout = timeout })
    return { status = "main-running", issued = attempt, selectedDistro = s.selectedDistro, attempt = attempt, op = op }, effects
end

-- One step of the dialog machine (`transitionWslDialog`).
function dialog.transition(s: DialogState, e: { [string]: any }): (DialogState, { Effect })
    if s.status == "disposed" then
        return s, {}
    end
    if e.type == "disposed" then
        local effects = if s.status == "main-running" or s.status == "catalog-installing" then { { type = "action.abort", attempt = s.attempt } } else {}
        return { status = "disposed", issued = s.issued }, effects
    end
    if s.status == "main" then
        if e.type == "distro.selected" then
            if s.selectedDistro == e.distro then
                return s, {}
            end
            return { status = "main", issued = s.issued, selectedDistro = e.distro }, {}
        elseif e.type == "catalog.opened" and not e.busy then
            return { status = "catalog", issued = s.issued, selectedDistro = s.selectedDistro, catalogTarget = e.defaultTarget }, {}
        elseif e.type == "wsl.install.requested" and not e.busy then
            return run(s, { kind = "install-wsl" }, INSTALL_TIMEOUT, {})
        elseif e.type == "distros.refresh.requested" and not e.busy then
            return run(s, { kind = "refresh-distros" }, ACTION_TIMEOUT, { { type = "probes.reset" } })
        elseif e.type == "primary.requested" and not e.disabled and e.distro and e.action then
            if e.action == "install-opencode" then
                return run(s, { kind = "install-opencode", distro = e.distro }, INSTALL_TIMEOUT, {})
            end
            return run(s, { kind = "add-server", distro = e.distro }, ACTION_TIMEOUT, {})
        end
        return s, {}
    end
    if s.status == "catalog" then
        if e.type == "catalog.closed" and not e.busy then
            return { status = "main", issued = s.issued, selectedDistro = s.selectedDistro }, { { type = "probes.reset" } }
        elseif e.type == "catalog.target.changed" then
            if s.catalogTarget == e.distro then
                return s, {}
            end
            return { status = "catalog", issued = s.issued, selectedDistro = s.selectedDistro, catalogTarget = e.distro }, {}
        elseif e.type == "catalog.install.requested" and not e.busy and e.distro then
            local attempt = s.issued + 1
            return { status = "catalog-installing", issued = attempt, selectedDistro = s.selectedDistro, catalogTarget = e.distro, attempt = attempt, distro = e.distro },
                {
                    { type = "probes.reset" },
                    { type = "action.run", attempt = attempt, action = { kind = "install-distro", distro = e.distro }, timeout = INSTALL_TIMEOUT },
                }
        end
        return s, {}
    end
    if s.status == "completed" then
        return s, {}
    end
    -- A running call ends in its own view: an installed distro returns to the list, a failed
    -- install stays in the catalog.
    local settledState: DialogState = if s.status == "main-running"
        then { status = "main", issued = s.issued, selectedDistro = s.selectedDistro }
        else { status = "catalog", issued = s.issued, selectedDistro = s.selectedDistro, catalogTarget = s.catalogTarget }
    -- The app half going away answers the call itself: the dialog shows WSL as unavailable,
    -- without a toast.
    if e.type == "availability.changed" then
        if e.available then
            return s, {}
        end
        return settledState, { { type = "action.abort", attempt = s.attempt } }
    end
    if (e.type ~= "action.succeeded" and e.type ~= "action.failed") or e.attempt ~= s.attempt then
        return s, {}
    end
    local abort = { type = "action.abort", attempt = e.attempt }
    if e.type == "action.failed" then
        return settledState, { abort, { type = "toast.error", cause = e.cause } }
    end
    if s.status == "catalog-installing" then
        return { status = "main", issued = s.issued, selectedDistro = s.selectedDistro }, { abort }
    end
    if s.op.kind == "add-server" then
        return { status = "completed", issued = s.issued, selectedDistro = s.selectedDistro }, { abort, { type = "dialog.close" } }
    end
    return settledState, { abort }
end

function dialog.probesInitial(): ProbeState
    return { status = "idle", issued = 0, failed = {} }
end

local function empty(t: { [string]: boolean }): boolean
    return next(t) == nil
end

local function schedule(previous: ProbeState?, issued: number, completed: string?, failed: { [string]: boolean }, input: any): (ProbeState, { Effect })
    if not input.available then
        if previous and previous.completed == nil and empty(previous.failed) then
            return previous, {}
        end
        return { status = "idle", issued = issued, failed = {} }, {}
    end
    local plan = input.plan
    local held = if completed ~= nil and (input.busy or (plan and plan.key == completed)) then completed else nil
    if not plan or plan.key == held or failed[plan.key] then
        if previous and previous.completed == held and previous.failed == failed then
            return previous, {}
        end
        return { status = "idle", issued = issued, completed = held, failed = failed }, {}
    end
    local attempt = issued + 1
    return { status = "running", issued = attempt, attempt = attempt, command = plan, failed = failed }, { { type = "probe.run", attempt = attempt, command = plan, timeout = PROBE_TIMEOUT } }
end

-- One step of the probe machine (`transitionWslProbes`).
function dialog.probeTransition(s: ProbeState, e: { [string]: any }): (ProbeState, { Effect })
    if s.status == "disposed" then
        return s, {}
    end
    if e.type == "disposed" then
        return { status = "disposed", issued = s.issued, failed = {} }, if s.status == "running" then { { type = "probe.abort", attempt = s.attempt } } else {}
    end
    -- Only the user asking to check again clears failures; the next state plans again.
    if e.type == "reset" then
        if empty(s.failed) then
            return s, {}
        end
        local copy = table.clone(s)
        copy.failed = {}
        return copy, {}
    end
    if s.status == "idle" then
        if e.type ~= "synced" then
            return s, {}
        end
        return schedule(s, s.issued, s.completed, s.failed, e)
    end
    if e.type == "synced" then
        if e.available then
            return s, {}
        end
        return { status = "idle", issued = s.issued, failed = {} }, { { type = "probe.abort", attempt = s.attempt } }
    end
    if e.attempt ~= s.attempt then
        return s, {}
    end
    local abort = { type = "probe.abort", attempt = e.attempt }
    if e.type == "probe.succeeded" then
        local nextState, effects = schedule(nil, s.issued, s.command.key, s.failed, e)
        table.insert(effects, 1, abort)
        return nextState, effects
    end
    local failed = table.clone(s.failed)
    failed[s.command.key] = true
    local nextState, effects = schedule(nil, s.issued, nil, failed, e)
    table.insert(effects, 1, { type = "probe.error", cause = e.cause })
    table.insert(effects, 1, abort)
    return nextState, effects
end

-- The controller for one window's dialog. `env` has `api` (the `wsl` contract's value or
-- nil), the services it uses, the catalog's search editor and `changed`, fired whenever what
-- the dialog shows moves.
function dialog.controller(env: any)
    -- Disposed until the dialog opens, as Desktop's dialog component mounts with it.
    local self = { state = { status = "disposed", issued = 0 } :: DialogState, probes = { status = "disposed", issued = 0, failed = {} } :: ProbeState }
    local runs: { [number]: { call: number, timer: thread } } = {}
    local probeRuns: { [number]: { call: number, timer: thread } } = {}
    local synced: string? = nil
    local dispatch: (e: { [string]: any }) -> ()
    local probeDispatch: (e: { [string]: any }) -> ()

    local function contract(): any
        return env.api()
    end

    local function current(): any
        local c = contract()
        if c then
            return c.state
        end
        return { runtime = { available = false, error = env.I18n:T("error.unavailable") }, installed = {}, online = {}, distroProbes = {}, opencodeChecks = {}, pendingRestart = false, servers = {}, hostJob = "idle", distroJobs = {} }
    end

    function self.model(probingAddable: boolean?): any
        local probing = if probingAddable == nil then self.probes.status == "running" and self.probes.command.kind == "addable" else probingAddable
        return model.build(current(), self.state, env.search:Text(), probing :: boolean)
    end

    local function input()
        local base = self.model(false)
        local c = contract()
        return { plan = model.plan(current(), base.view, base.busy, base.selected, base.addable), busy = base.busy, available = c ~= nil }
    end

    local function toastError(cause: string?)
        env.Toast:Error(env.I18n:T("common.requestFailed"), cause)
    end

    local function request(kind: string, arg: any): number?
        local c = contract()
        if not c then
            return nil
        end
        return c.request(kind, arg)
    end

    -- Calls that settled since they were made: their outcomes from the contract's `results`.
    local function collect()
        local c = contract()
        if not c then
            return
        end
        for attempt, r in runs do
            local result = c.results[tostring(r.call)]
            if result then
                if result.error then
                    dispatch({ type = "action.failed", attempt = attempt, cause = result.error })
                else
                    dispatch({ type = "action.succeeded", attempt = attempt })
                end
            end
        end
        for attempt, r in probeRuns do
            local result = c.results[tostring(r.call)]
            if result then
                local i = input()
                if result.error then
                    probeDispatch({ type = "probe.failed", attempt = attempt, cause = result.error, plan = i.plan, busy = i.busy, available = i.available })
                else
                    probeDispatch({ type = "probe.succeeded", attempt = attempt, plan = i.plan, busy = i.busy, available = i.available })
                end
            end
        end
    end

    local function runEffect(effect: Effect)
        local t = effect.type
        if t == "probes.reset" then
            probeDispatch({ type = "reset" })
        elseif t == "dialog.close" then
            env.Dialog:Close()
        elseif t == "toast.error" then
            toastError(effect.cause)
        elseif t == "action.abort" then
            local r = runs[effect.attempt]
            runs[effect.attempt] = nil
            if r then
                task.cancel(r.timer)
            end
        elseif t == "action.run" then
            local attempt = effect.attempt
            local a = effect.action
            local kind = if a.kind == "install-wsl" then "installWsl" elseif a.kind == "refresh-distros" then "refreshDistros" elseif a.kind == "install-opencode" then "installOpencode" elseif a.kind == "add-server" then "addServer" else "installDistro"
            local call = request(kind, a.distro)
            if not call then
                return dispatch({ type = "action.failed", attempt = attempt, cause = env.I18n:T("error.unavailable") })
            end
            -- A call that timed out has no cause; its toast says only that the request failed.
            runs[attempt] = { call = call, timer = task.delay(effect.timeout, function()
                runs[attempt] = nil
                dispatch({ type = "action.failed", attempt = attempt })
            end) }
            collect()
        elseif t == "probe.error" then
            toastError(effect.cause)
        elseif t == "probe.abort" then
            local r = probeRuns[effect.attempt]
            probeRuns[effect.attempt] = nil
            if r then
                task.cancel(r.timer)
            end
        elseif t == "probe.run" then
            local attempt = effect.attempt
            local command = effect.command
            local call = if command.kind == "addable" then request("probeAddable", command.distros) else request(command.action, nil)
            if not call then
                return
            end
            probeRuns[attempt] = { call = call, timer = task.delay(effect.timeout, function()
                probeRuns[attempt] = nil
                local i = input()
                probeDispatch({ type = "probe.failed", attempt = attempt, cause = "WSL probe timed out", plan = i.plan, busy = i.busy, available = i.available })
            end) }
            collect()
        end
    end

    -- Offers the probe machine what the dialog shows now, when its plan, busy or availability moved.
    local function sync()
        local i = input()
        local key = `{if i.plan then i.plan.key else ""}|{i.busy}|{i.available}`
        if key == synced then
            return
        end
        synced = key
        probeDispatch({ type = "synced", plan = i.plan, busy = i.busy, available = i.available })
    end

    local queue: { { machine: string, event: { [string]: any } } } = {}
    local draining = false
    local function drain()
        if draining then
            return
        end
        draining = true
        while #queue > 0 do
            local item = table.remove(queue, 1) :: any
            local effects
            if item.machine == "dialog" then
                self.state, effects = dialog.transition(self.state, item.event)
            else
                self.probes, effects = dialog.probeTransition(self.probes, item.event)
            end
            for _, effect in effects do
                runEffect(effect)
            end
        end
        draining = false
        sync()
        env.changed()
    end

    dispatch = function(e)
        table.insert(queue, { machine = "dialog", event = e })
        drain()
    end
    probeDispatch = function(e)
        table.insert(queue, { machine = "probes", event = e })
        drain()
    end

    self.dispatch = dispatch

    -- The app half's state or results moved.
    function self.refresh()
        collect()
        sync()
        env.changed()
    end

    function self.availability(available: boolean)
        dispatch({ type = "availability.changed", available = available })
    end

    -- The dialog opens afresh.
    function self.open()
        for attempt in runs do
            runEffect({ type = "action.abort", attempt = attempt })
        end
        for attempt in probeRuns do
            runEffect({ type = "probe.abort", attempt = attempt })
        end
        self.state = dialog.initial()
        self.probes = dialog.probesInitial()
        synced = nil
        sync()
    end

    -- The dialog went away: its call and probe end with it.
    function self.dispose()
        dispatch({ type = "disposed" })
        probeDispatch({ type = "disposed" })
    end

    return self
end

return dialog
