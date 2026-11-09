--!strict
-- The pairing page (Desktop's `machine.ts`): the remote access setting, the Pair a device
-- dialog that turns it on, and the display sleep setting. One transition owns them, so the dialog
-- follows every change of remote access in the same event. `issued` is the last attempt handed
-- out; each request and timer carries its attempt, and a stale one changes nothing. `step`
-- changes the state and returns the effects.

local machine = {}

-- Seconds.
machine.REMOTE_READ_TIMEOUT = 15
-- How many times a failed read of either setting is tried again.
machine.REMOTE_READ_RETRIES = 3
-- Turning remote access on the first time creates the tunnel, which can take a minute.
machine.REMOTE_CHANGE_TIMEOUT = 180
-- The service attaches the tunnel after it starts: asked again this many times, a second apart,
-- as `opencode pair --remote` waits.
machine.TUNNEL_RETRIES = 180
machine.TUNNEL_RETRY_DELAY = 1
machine.TUNNEL_TIMEOUT = 10
machine.CODE_TIMEOUT = 15
machine.SCREEN_TIMEOUT = 15

-- The wait before read retry `failures`: 1, 2, then 4 seconds.
function machine.readDelay(failures: number): number
    return math.min(2 ^ math.min(failures, 5), 30)
end

-- `pending` while the app half hasn't answered, `inactive` once it went away.
export type Setting =
    { status: "pending" }
    | { status: "inactive" }
    | { status: "loading", attempt: number, waiting: boolean, failures: number }
    | { status: "ready", enabled: boolean }
    | { status: "changing", attempt: number, enabled: boolean, restart: boolean, previous: boolean? }
    | { status: "failed", previous: boolean? }

export type Dialog =
    { status: "closed" }
    -- Neither the local server nor the service can set up pairing.
    | { status: "unavailable" }
    -- Remote access is being read or turned on.
    | { status: "turning-on" }
    -- Remote access is on; asking the server for its tunnel until it advertises one.
    | { status: "tunnel", attempt: number, waiting: boolean, retries: number }
    | { status: "code", attempt: number, tunnel: string }
    | { status: "ready", tunnel: string, code: string, url: string, expiresIn: number, loadedAt: number, refreshing: number? }
    | { status: "code-error", tunnel: string }
    | { status: "failed", tunnelFailed: boolean }

export type State = { issued: number, server: boolean, remote: Setting, screen: Setting, dialog: Dialog }
export type Event = { type: string, [string]: any }
export type Effect = { type: string, [string]: any }

function machine.create(server: boolean): State
    return { issued = 0, server = server, remote = { status = "pending" }, screen = { status = "pending" }, dialog = { status = "closed" } }
end

local function nextAttempt(s: State): number
    s.issued += 1
    return s.issued
end

local function attemptOf(d: any): number?
    if d.status == "tunnel" or d.status == "code" then
        return d.attempt
    elseif d.status == "ready" then
        return d.refreshing
    end
    return nil
end

local function abortDialog(s: State, out: { Effect })
    local a = attemptOf(s.dialog)
    if a then
        table.insert(out, { type = "abort", attempt = a })
    end
end

local function toDialog(s: State, out: { Effect }, d: Dialog): { Effect }
    local cur = s.dialog :: any
    local nd = d :: any
    if cur.status == nd.status and cur.tunnelFailed == nd.tunnelFailed and (cur.status == "closed" or cur.status == "unavailable" or cur.status == "turning-on" or cur.status == "failed") then
        return out
    end
    abortDialog(s, out)
    s.dialog = d
    return out
end

local function startTunnel(s: State, out: { Effect }): { Effect }
    abortDialog(s, out)
    local attempt = nextAttempt(s)
    s.dialog = { status = "tunnel", attempt = attempt, waiting = false, retries = 0 }
    table.insert(out, { type = "poll-tunnel", attempt = attempt, timeout = machine.TUNNEL_TIMEOUT })
    return out
end

-- Turns remote access on or off; the caller checked that no read or change is in flight.
local function change(s: State, out: { Effect }, enabled: boolean, restart: boolean): { Effect }
    local r = s.remote :: any
    local previous: boolean?
    if r.status == "ready" then
        previous = r.enabled
    elseif r.status == "failed" then
        previous = r.previous
    else
        return out
    end
    local attempt = nextAttempt(s)
    s.remote = { status = "changing", attempt = attempt, enabled = enabled, restart = restart, previous = previous }
    table.insert(out, { type = "set-remote", attempt = attempt, enabled = enabled, restart = restart, timeout = machine.REMOTE_CHANGE_TIMEOUT })
    return out
end

local function setUp(s: State, out: { Effect }): { Effect }
    local r = s.remote :: any
    if r.status == "failed" then
        return toDialog(s, out, { status = "failed", tunnelFailed = false })
    elseif r.status == "ready" and r.enabled then
        return startTunnel(s, out)
    elseif r.status == "ready" then
        return toDialog(s, change(s, out, true, false), { status = "turning-on" })
    end
    return toDialog(s, out, { status = "turning-on" })
end

-- Moves an open dialog to the phase that follows from remote access and the server.
local function settle(s: State, out: { Effect }): { Effect }
    local d = s.dialog :: any
    local r = s.remote :: any
    if d.status == "closed" then
        return out
    end
    -- Pending main is still starting; an app half that went away keeps pairing from turning on.
    if not s.server or r.status == "inactive" then
        return toDialog(s, out, { status = "unavailable" })
    end
    if d.status == "tunnel" or d.status == "code" or d.status == "ready" or d.status == "code-error" then
        if r.status == "failed" then
            return toDialog(s, out, { status = "failed", tunnelFailed = false })
        elseif r.status == "changing" or (r.status == "ready" and not r.enabled) then
            return setUp(s, out)
        end
        -- The code needs only the server.
        return out
    end
    if d.status == "failed" then
        if r.status == "changing" then
            return toDialog(s, out, { status = "turning-on" })
        elseif r.status == "failed" then
            return toDialog(s, out, { status = "failed", tunnelFailed = false })
        elseif r.status == "ready" and r.enabled and not d.tunnelFailed then
            -- A tunnel that never connected waits for Try again, which restarts the service.
            return startTunnel(s, out)
        end
        return out
    end
    return setUp(s, out)
end

local function abortSetting(set: any, out: { Effect })
    if set.status == "loading" or set.status == "changing" then
        table.insert(out, { type = "abort", attempt = set.attempt })
    end
end

local function remaining(s: State, now: number): number
    local d = s.dialog :: any
    if d.status ~= "ready" then
        return 0
    end
    -- The clock can lag a fresh code by up to a second; a negative age would show one second too many.
    return math.max(0, d.expiresIn - math.floor(math.max(0, now - d.loadedAt) / 1000))
end
machine.remaining = remaining

local function refresh(s: State): { Effect }
    local attempt = nextAttempt(s)
    local d = s.dialog :: any
    d.refreshing = attempt
    return { { type = "fetch-code", attempt = attempt, timeout = machine.CODE_TIMEOUT } }
end

local function currentCode(s: State, attempt: number): boolean
    local d = s.dialog :: any
    if d.status == "code" then
        return d.attempt == attempt
    elseif d.status == "ready" then
        return d.refreshing == attempt
    end
    return false
end

local function tunnelOf(s: State): string
    local d = s.dialog :: any
    return d.tunnel or ""
end

-- One setting's read, with its retries: `remote` or `screen`.
local function readDone(s: State, which: string, attempt: number, enabled: boolean?): { Effect }
    local set = (s :: any)[which]
    if set.status ~= "loading" or set.waiting or set.attempt ~= attempt then
        return {}
    end
    local out: { Effect } = { { type = "abort", attempt = attempt } }
    if enabled ~= nil then
        (s :: any)[which] = { status = "ready", enabled = enabled }
    elseif set.failures >= machine.REMOTE_READ_RETRIES then
        (s :: any)[which] = { status = "failed" }
    else
        local a = nextAttempt(s);
        (s :: any)[which] = { status = "loading", attempt = a, waiting = true, failures = set.failures + 1 }
        table.insert(out, { type = "wait", kind = which, attempt = a, delay = machine.readDelay(set.failures) })
        return out
    end
    return if which == "remote" then settle(s, out) else out
end

local function read(s: State, which: string, failures: number, out: { Effect }): { Effect }
    local a = nextAttempt(s);
    (s :: any)[which] = { status = "loading", attempt = a, waiting = false, failures = failures }
    table.insert(out, { type = if which == "remote" then "read-remote" else "read-screen", attempt = a, timeout = if which == "remote" then machine.REMOTE_READ_TIMEOUT else machine.SCREEN_TIMEOUT })
    return out
end

function machine.step(s: State, e: Event): { Effect }
    local t = e.type
    local d = s.dialog :: any
    if t == "main.changed" then
        -- Both settings are read afresh from each app half that answers.
        local out: { Effect } = {}
        abortSetting(s.remote, out)
        abortSetting(s.screen, out)
        if e.active then
            read(s, "remote", 0, out)
            read(s, "screen", 0, out)
        else
            s.remote = { status = if e.pending then "pending" else "inactive" } :: any
            s.screen = { status = if e.pending then "pending" else "inactive" } :: any
        end
        return settle(s, out)
    elseif t == "server.changed" then
        if s.server == e.available then
            return {}
        end
        s.server = e.available
        return settle(s, {})
    elseif t == "read.done" then
        return readDone(s, "remote", e.attempt, e.enabled)
    elseif t == "screen.read.done" then
        return readDone(s, "screen", e.attempt, e.enabled)
    elseif t == "wait.ended" and (e.kind == "remote" or e.kind == "screen") then
        local set = (s :: any)[e.kind]
        if set.status ~= "loading" or not set.waiting or set.attempt ~= e.attempt then
            return {}
        end
        return read(s, e.kind, set.failures, { { type = "abort", attempt = e.attempt } })
    elseif t == "toggled" then
        local r = s.remote :: any
        -- A change reads the setting before it writes, so the switch also works after a read
        -- that never answered.
        if (r.status == "ready" and r.enabled ~= e.enabled) or r.status == "failed" then
            return settle(s, change(s, {}, e.enabled, false))
        end
        return {}
    elseif t == "change.done" then
        local r = s.remote :: any
        if r.status ~= "changing" or r.attempt ~= e.attempt then
            return {}
        end
        s.remote = if e.ok then { status = "ready", enabled = r.enabled } else { status = "failed", previous = r.previous }
        return settle(s, { { type = "abort", attempt = e.attempt } })
    elseif t == "screen.retry" then
        if s.screen.status ~= "failed" then
            return {}
        end
        return read(s, "screen", 0, {})
    elseif t == "screen.toggled" then
        local c = s.screen :: any
        local previous: boolean?
        if c.status == "ready" then
            if c.enabled == e.enabled then
                return {}
            end
            previous = c.enabled
        elseif c.status == "failed" then
            previous = c.previous
        else
            return {}
        end
        local a = nextAttempt(s)
        s.screen = { status = "changing", attempt = a, enabled = e.enabled, restart = false, previous = previous }
        return { { type = "set-screen", attempt = a, enabled = e.enabled, timeout = machine.SCREEN_TIMEOUT } }
    elseif t == "screen.change.done" then
        local c = s.screen :: any
        if c.status ~= "changing" or c.attempt ~= e.attempt then
            return {}
        end
        s.screen = if e.ok then { status = "ready", enabled = c.enabled } else { status = "failed", previous = c.previous }
        return { { type = "abort", attempt = e.attempt } }
    elseif t == "dialog.opened" then
        if d.status ~= "closed" then
            return {}
        end
        s.dialog = { status = "turning-on" }
        local out: { Effect } = { { type = "open-dialog" } }
        -- Every pairing goes through remote access, so opening the dialog tries again after a failure.
        if s.remote.status == "failed" then
            change(s, out, true, false)
        end
        return settle(s, out)
    elseif t == "dialog.closed" then
        return toDialog(s, {}, { status = "closed" })
    elseif t == "retry" then
        if d.status ~= "failed" then
            return {}
        end
        local tunnelFailed = d.tunnelFailed
        s.dialog = { status = "turning-on" }
        local out: { Effect } = {}
        -- A tunnel that never connected comes back when the service starts it again.
        if s.remote.status == "ready" or s.remote.status == "failed" then
            change(s, out, true, tunnelFailed)
        end
        return settle(s, out)
    elseif t == "tunnel.found" then
        if d.status ~= "tunnel" or d.waiting or d.attempt ~= e.attempt then
            return {}
        end
        local a = nextAttempt(s)
        s.dialog = { status = "code", attempt = a, tunnel = e.tunnel }
        return { { type = "abort", attempt = e.attempt }, { type = "fetch-code", attempt = a, timeout = machine.CODE_TIMEOUT } }
    elseif t == "tunnel.missing" then
        if d.status ~= "tunnel" or d.waiting or d.attempt ~= e.attempt then
            return {}
        end
        if d.retries >= machine.TUNNEL_RETRIES then
            s.dialog = { status = "failed", tunnelFailed = true }
            return { { type = "abort", attempt = e.attempt } }
        end
        local a = nextAttempt(s)
        s.dialog = { status = "tunnel", attempt = a, waiting = true, retries = d.retries + 1 }
        return { { type = "abort", attempt = e.attempt }, { type = "wait", kind = "tunnel", attempt = a, delay = machine.TUNNEL_RETRY_DELAY } }
    elseif t == "wait.ended" then
        if d.status ~= "tunnel" or not d.waiting or d.attempt ~= e.attempt then
            return {}
        end
        local a = nextAttempt(s)
        s.dialog = { status = "tunnel", attempt = a, waiting = false, retries = d.retries }
        return { { type = "abort", attempt = e.attempt }, { type = "poll-tunnel", attempt = a, timeout = machine.TUNNEL_TIMEOUT } }
    elseif t == "code.loaded" or t == "code.failed" then
        if not currentCode(s, e.attempt) then
            return {}
        end
        local tunnel = tunnelOf(s)
        if t == "code.failed" then
            s.dialog = { status = "code-error", tunnel = tunnel }
        else
            local base = string.gsub(tunnel, "/+$", "")
            s.dialog = { status = "ready", tunnel = tunnel, code = e.code, url = `{base}/auth/connect/{e.code}`, expiresIn = e.expiresIn, loadedAt = e.now, refreshing = nil }
        end
        return { { type = "abort", attempt = e.attempt } }
    elseif t == "code.refresh" then
        if d.status == "code-error" then
            local a = nextAttempt(s)
            s.dialog = { status = "code", attempt = a, tunnel = d.tunnel }
            return { { type = "fetch-code", attempt = a, timeout = machine.CODE_TIMEOUT } }
        elseif d.status == "ready" and d.refreshing == nil then
            return refresh(s)
        end
        return {}
    elseif t == "tick" then
        -- Codes are single-use, so the link is replaced as soon as the server forgets the code.
        if d.status == "ready" and d.refreshing == nil and remaining(s, e.now) == 0 then
            return refresh(s)
        end
        return {}
    end
    return {}
end

-- What the page shows (`viewPairingPage`).
function machine.page(s: State)
    local r = s.remote :: any
    local c = s.screen :: any
    local mainActive = r.status ~= "pending" and r.status ~= "inactive"
    local function checked(set: any): boolean
        return ((set.status == "changing" or set.status == "ready") and set.enabled) or (set.status == "failed" and set.previous == true)
    end
    return {
        mainActive = mainActive,
        pairDisabled = not mainActive or not s.server or (r.status == "changing" and not r.enabled),
        -- A change shows its target; a failed one shows the setting it left.
        remoteChecked = checked(r),
        remoteDisabled = r.status ~= "ready" and r.status ~= "failed",
        remoteError = r.status == "failed",
        screenChecked = checked(c),
        -- A failed read or change keeps the switch enabled, so flipping it tries again.
        screenDisabled = c.status ~= "ready" and c.status ~= "failed",
        screenError = c.status == "failed",
        unavailable = r.status == "inactive" or not s.server,
    }
end

return machine
