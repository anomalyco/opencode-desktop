--!strict
-- The SSH main side's state machine (Desktop's `main-machine.ts`): each server's connection,
-- fenced by the attempt that started it. An interactive attempt (a window asked) shows its
-- sign-in prompts; a background one (a saved server restored at launch) pauses in
-- `authentication` at its first. A saved server that fails for a reason another try can fix
-- reconnects by itself, five times, 3 s apart. `step` changes the state and returns the effects.

local machine = {}

machine.RETRY_LIMIT = 5
machine.RETRY_DELAY = 3

export type Config = { id: string, target: string, name: string }
export type Http = { url: string, password: string }
export type Prompt = { id: string, text: string, confirm: boolean }

-- Where a saved server returns when its attempt is cancelled.
export type Resting =
    { kind: "disconnected" }
    | { kind: "authentication" }
    | { kind: "failed", code: string, detail: string, failures: number }

-- `connecting` and `authenticating` run an attempt; `failed` may hold a scheduled retry.
export type Phase =
    { kind: "disconnected" }
    | { kind: "authentication" }
    | { kind: "connecting", attempt: number, interactive: boolean, failures: number, resting: Resting }
    | { kind: "authenticating", attempt: number, failures: number, resting: Resting, prompt: Prompt }
    | { kind: "ready", attempt: number, http: Http }
    | { kind: "failed", code: string, detail: string, failures: number, retry: number? }

export type Record = { config: Config, saved: Config?, destination: string?, phase: Phase }
export type State = { issued: number, servers: { Record } }
export type Event = { type: string, [string]: any }
export type Effect = { type: string, [string]: any }

function machine.create(configs: { Config }): State
    local servers = {}
    for _, c in configs do
        table.insert(servers, { config = c, saved = table.clone(c), phase = { kind = "disconnected" } })
    end
    return { issued = 0, servers = servers }
end

-- What windows see of a server (`SshItem.stage`).
function machine.stage(r: Record): string
    local k = r.phase.kind
    if k == "authenticating" then
        return "authentication"
    end
    return k
end

function machine.prompt(r: Record): Prompt?
    local p = r.phase :: any
    return if p.kind == "authenticating" then p.prompt else nil
end

function machine.http(r: Record): Http?
    local p = r.phase :: any
    return if p.kind == "ready" then p.http else nil
end

function machine.error(r: Record): string?
    local p = r.phase :: any
    return if p.kind == "failed" then p.code else nil
end

local function active(phase: Phase): (number?, number, Resting?)
    local p = phase :: any
    if p.kind == "connecting" or p.kind == "authenticating" then
        return p.attempt, p.failures, p.resting
    end
    return nil, 0, nil
end

local function find(s: State, id: string): number?
    for i, r in s.servers do
        if r.config.id == id then
            return i
        end
    end
    return nil
end

local function save(s: State): Effect
    local list = {}
    for _, r in s.servers do
        if r.saved then
            table.insert(list, { id = r.saved.id, target = r.saved.target, name = r.saved.name })
        end
    end
    return { type = "save", configs = list }
end

-- Stops whatever runs for a server: its connect, its tunnel, its retry.
local function halt(phase: Phase): { Effect }
    local p = phase :: any
    if p.kind == "connecting" or p.kind == "authenticating" or p.kind == "ready" then
        return { { type = "attempt.stop", attempt = p.attempt } }
    elseif p.kind == "failed" and p.retry then
        return { { type = "attempt.stop", attempt = p.retry } }
    end
    return {}
end

local function restingOf(phase: Phase): Resting
    local p = phase :: any
    if p.kind == "connecting" or p.kind == "authenticating" then
        return p.resting
    elseif p.kind == "authentication" then
        return { kind = "authentication" }
    elseif p.kind == "failed" then
        return { kind = "failed", code = p.code, detail = p.detail, failures = p.failures }
    end
    return { kind = "disconnected" }
end

local function rest(r: Resting): Phase
    local x = r :: any
    if x.kind == "failed" then
        return { kind = "failed", code = x.code, detail = x.detail, failures = x.failures, retry = nil }
    end
    return x
end

local function trim(s: string): string
    return (string.match(s, "^%s*(.-)%s*$") or "")
end

local function failed(s: State, i: number, reported: string, detail: string, failures: number): { Effect }
    local code = reported
    if string.find(detail, "REMOTE HOST IDENTIFICATION HAS CHANGED", 1, true) or string.find(detail, "Host key verification failed", 1, true) then
        code = "host-key"
    elseif string.find(detail, "spawn", 1, true) and string.find(detail, "ENOENT", 1, true) then
        code = "ssh-missing"
    end
    -- A failed attempt shows the saved server again, not the target it tried.
    local r = s.servers[i]
    r.config = if r.saved then table.clone(r.saved) else r.config
    -- Only a saved server reconnects by itself, and only from failures another try can fix.
    local fixable = not (code == "input" or code == "platform" or code == "unpublished" or code == "host-key" or code == "ssh-missing")
    local retry = r.saved ~= nil and failures < machine.RETRY_LIMIT and fixable and not string.find(detail, "Permission denied", 1, true)
    if not retry then
        r.phase = { kind = "failed", code = code, detail = detail, failures = failures, retry = nil }
        return {}
    end
    s.issued += 1
    local attempt = s.issued
    r.phase = { kind = "failed", code = code, detail = detail, failures = failures, retry = attempt }
    return { { type = "retry.schedule", id = r.config.id, attempt = attempt } }
end

function machine.step(s: State, e: Event): { Effect }
    local t = e.type
    if t == "start" then
        local config: Config = e.config
        local id = config.id
        if #id == 0 or #id > 80 or not string.match(id, "^[%w%-]+$") then
            return {}
        end
        local at = find(s, id)
        local phase: Phase = if at then s.servers[at].phase else { kind = "disconnected" }
        -- A background restore joins a server already connecting or up.
        if e.background and (active(phase) ~= nil or phase.kind == "ready") then
            return {}
        end
        s.issued += 1
        local attempt = s.issued
        local clean: Config = { id = id, target = trim(config.target), name = trim(config.name) }
        local failures = if phase.kind == "failed" and e.background then (phase :: any).failures else 0
        local record: Record = {
            config = clean,
            saved = if at then s.servers[at].saved else nil,
            destination = if at then s.servers[at].destination else nil,
            phase = { kind = "connecting", attempt = attempt, interactive = not e.background, failures = failures, resting = restingOf(phase) },
        }
        if at then
            s.servers[at] = record
        else
            table.insert(s.servers, record)
        end
        local effects = halt(phase)
        table.insert(effects, { type = "attempt.start", id = id, attempt = attempt, config = clean })
        return effects
    elseif t == "cancel" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local attempt, _, resting = active(s.servers[i].phase)
        if not attempt then
            return {}
        end
        -- A server the window was adding is gone with its attempt.
        local saved = s.servers[i].saved
        if not saved then
            table.remove(s.servers, i)
        else
            s.servers[i].config = table.clone(saved)
            s.servers[i].phase = rest(resting :: Resting)
        end
        return { { type = "attempt.stop", attempt = attempt } }
    elseif t == "forget" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local r = table.remove(s.servers, i) :: Record
        local effects = halt(r.phase)
        if r.saved then
            table.insert(effects, save(s))
        end
        return effects
    elseif t == "retry.due" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local p = s.servers[i].phase :: any
        if p.kind ~= "failed" or p.retry ~= e.attempt then
            return {}
        end
        s.issued += 1
        local attempt = s.issued
        s.servers[i].phase = {
            kind = "connecting",
            attempt = attempt,
            interactive = false,
            failures = p.failures,
            resting = { kind = "failed", code = p.code, detail = p.detail, failures = p.failures },
        }
        return { { type = "attempt.start", id = e.id, attempt = attempt, config = s.servers[i].config } }
    elseif t == "respond" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local p = s.servers[i].phase :: any
        if p.kind == "authenticating" and p.prompt.id == e.prompt then
            return { { type = "attempt.respond", attempt = p.attempt, prompt = e.prompt, value = e.value } }
        end
        return {}
    elseif t == "failed" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local p = s.servers[i].phase :: any
        local failures
        if p.kind == "ready" and p.attempt == e.attempt then
            failures = 0
        else
            local a, f = active(p)
            if a ~= e.attempt then
                return {}
            end
            failures = f
        end
        return failed(s, i, e.code, e.detail, failures + 1)
    elseif t == "destination.resolved" then
        local i = find(s, e.id)
        if i and active(s.servers[i].phase) == e.attempt then
            s.servers[i].destination = e.destination
        end
        return {}
    elseif t == "prompt.received" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local p = s.servers[i].phase :: any
        -- A background attempt has nobody to answer: the server waits for the user to sign in.
        if p.kind == "connecting" and p.attempt == e.attempt and not p.interactive then
            s.servers[i].phase = { kind = "authentication" }
            return { { type = "attempt.stop", attempt = e.attempt } }
        end
        if (p.kind == "connecting" or p.kind == "authenticating") and p.attempt == e.attempt then
            s.servers[i].phase = { kind = "authenticating", attempt = e.attempt, failures = p.failures, resting = p.resting, prompt = e.prompt }
        end
        return {}
    elseif t == "prompt.cleared" then
        local i = find(s, e.id)
        if not i then
            return {}
        end
        local p = s.servers[i].phase :: any
        if p.kind == "authenticating" and p.attempt == e.attempt and p.prompt.id == e.prompt then
            s.servers[i].phase = { kind = "connecting", attempt = e.attempt, interactive = true, failures = p.failures, resting = p.resting }
        end
        return {}
    elseif t == "ready" then
        local i = find(s, e.id)
        if not i or active(s.servers[i].phase) ~= e.attempt then
            return {}
        end
        local r = s.servers[i]
        r.saved = table.clone(r.config)
        r.phase = { kind = "ready", attempt = e.attempt, http = e.http }
        return { save(s) }
    end
    return {}
end

-- What windows see (`SshState.servers`).
function machine.published(s: State)
    local list = {}
    for _, r in s.servers do
        local p = r.phase :: any
        table.insert(list, {
            config = r.config,
            saved = r.saved ~= nil,
            destination = r.destination,
            stage = machine.stage(r),
            http = machine.http(r),
            prompt = machine.prompt(r),
            retrying = p.kind == "failed" and p.retry ~= nil,
            detail = p.detail or "",
            error = machine.error(r),
            attempt = active(p),
        })
    end
    return list
end

return machine
