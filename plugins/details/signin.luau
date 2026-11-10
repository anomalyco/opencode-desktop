--!strict
-- An MCP sign-in that also accepts the redirect URL pasted from the browser, for when the browser
-- cannot reach the machine running the server (`@opencode/ui/overlays/mcp-sign-in/machine.ts`).
-- Requests take attempt numbers from `issued`, so a result whose attempt the state no longer
-- holds is stale.

local signin = {}

-- Delay between status polls while the attempt is pending or the server is unreachable.
signin.POLL = 1
-- How long the MCP list reload after a sign-in may run before it shows a retryable error.
signin.SYNC_TIMEOUT = 30

export type Outcome =
    { status: "pending" }
    | { status: "complete" }
    | { status: "failed", message: string }
    | { status: "expired" }
    -- The status request failed; the attempt may still be open until its lifetime ends.
    | { status: "unreachable", message: string }

export type EntryError = { type: "required" } | { type: "rejected", message: string }

export type Failure =
    { type: "auth-expired" }
    | { type: "auth-failed", message: string }
    -- Signed in, but reloading the MCP list failed; a retry reloads it again.
    | { type: "sync-failed", message: string }
    | { type: "sync-timed-out" }

export type State =
    {
        status: "waiting",
        url: string,
        issued: number,
        pollAttempt: number,
        value: string,
        -- The server took a pasted redirect URL and finishes the sign-in; the poll reports how.
        pasteAccepted: boolean,
        entryError: EntryError?,
        -- The last status request failed; shown if the lifetime ends before the server answers.
        unreachableError: string?,
    }
    | {
        status: "submitting",
        url: string,
        issued: number,
        pollAttempt: number,
        submitAttempt: number,
        value: string,
        unreachableError: string?,
    }
    -- Signed in; the MCP list reloads before the dialog closes.
    | { status: "completing", url: string, issued: number, syncAttempt: number }
    | { status: "failed", url: string, issued: number, failure: Failure }
    | { status: "done" }
    | { status: "disposed" }

export type Event =
    { type: "browser.open" }
    | { type: "paste.changed", value: string }
    | { type: "paste.submitted" }
    | { type: "polled", pollAttempt: number, outcome: Outcome }
    | { type: "submit.succeeded", submitAttempt: number }
    | { type: "submit.failed", submitAttempt: number, message: string }
    -- The attempt's lifetime ended before the server reported an outcome.
    | { type: "waiting.timed-out" }
    | { type: "sync.succeeded", syncAttempt: number }
    | { type: "sync.failed", syncAttempt: number, message: string }
    | { type: "sync.timed-out", syncAttempt: number }
    | { type: "retry.requested" }
    | { type: "disposed" }

export type Effect =
    { type: "deadline.start", delay: number }
    | { type: "deadline.clear" }
    -- Drops the poll in flight or scheduled, then polls the attempt's status after `delay`.
    | { type: "poll", pollAttempt: number, delay: number }
    | { type: "poll.abort" }
    | { type: "submit", submitAttempt: number, code: string }
    | { type: "submit.abort" }
    -- Reloads the MCP list, reporting back within `timeout`.
    | { type: "sync", syncAttempt: number, timeout: number }
    | { type: "sync.clear" }
    -- Reloads the MCP list without reporting back, for a sign-in the server finishes later.
    | { type: "refresh" }
    | { type: "open-external", url: string }
    -- Ends the attempt on the server, so a late browser callback cannot connect the server.
    | { type: "cancel" }
    | { type: "close" }

function signin.start(url: string, lifetime: number): (State, { Effect })
    return {
        status = "waiting",
        url = url,
        issued = 1,
        pollAttempt = 1,
        value = "",
        pasteAccepted = false,
        entryError = nil,
        unreachableError = nil,
    },
        { { type = "deadline.start", delay = math.max(0, lifetime) }, { type = "poll", pollAttempt = 1, delay = 0 } }
end

export type View = {
    status: string,
    failure: Failure?,
    canRetry: boolean,
    showOpenBrowser: boolean,
    showForm: boolean,
    value: string,
    entryError: EntryError?,
    locked: boolean,
    submitting: boolean,
}

-- `selectMcpSignInView`.
function signin.view(state: State): View
    local failure = if state.status == "failed" then state.failure else nil
    local canRetry = failure ~= nil and (failure.type == "sync-failed" or failure.type == "sync-timed-out")
    local showForm = state.status == "waiting" or state.status == "submitting"
    return {
        status = state.status,
        failure = failure,
        canRetry = canRetry,
        -- Once the server reports the sign-in done, the browser has nothing left to do.
        showOpenBrowser = showForm
            or (failure ~= nil and (failure.type == "auth-expired" or failure.type == "auth-failed")),
        showForm = showForm,
        value = if state.status == "waiting" or state.status == "submitting" then state.value else "",
        entryError = if state.status == "waiting" then state.entryError else nil,
        locked = state.status == "submitting" or (state.status == "waiting" and state.pasteAccepted),
        submitting = state.status == "submitting",
    }
end

-- Stops the requests and timers of a sign-in still waiting for the server.
local function stop(state: State): { Effect }
    local effects: { Effect } = { { type = "deadline.clear" }, { type = "poll.abort" } }
    if state.status == "submitting" then
        table.insert(effects, { type = "submit.abort" })
    end
    return effects
end

local function sync(state: State): (State, { Effect })
    local syncAttempt = (state :: any).issued + 1
    return { status = "completing", url = (state :: any).url, issued = syncAttempt, syncAttempt = syncAttempt },
        { { type = "sync", syncAttempt = syncAttempt, timeout = signin.SYNC_TIMEOUT } }
end

local function polled(state: State, event: any): (State, { Effect })
    local current = state :: any
    if event.pollAttempt ~= current.pollAttempt then
        return state, {}
    end
    local outcome = event.outcome
    -- A failed request says nothing about the attempt, which may still be pending, so polling
    -- goes on until it expires.
    if outcome.status == "pending" or outcome.status == "unreachable" then
        local pollAttempt = current.issued + 1
        local next = table.clone(current)
        next.issued = pollAttempt
        next.pollAttempt = pollAttempt
        next.unreachableError = if outcome.status == "unreachable" then outcome.message else nil
        return next, { { type = "poll", pollAttempt = pollAttempt, delay = signin.POLL } }
    end
    if outcome.status == "complete" then
        local next, effects = sync(state)
        local all = stop(state)
        for _, e in effects do
            table.insert(all, e)
        end
        return next, all
    end
    local failure: Failure = if outcome.status == "expired"
        then { type = "auth-expired" }
        else { type = "auth-failed", message = outcome.message }
    return { status = "failed", url = current.url, issued = current.issued, failure = failure }, stop(state)
end

-- `transitionMcpSignIn`.
function signin.step(state: State, event: Event): (State, { Effect })
    if state.status == "disposed" then
        return state, {}
    end
    if event.type == "disposed" then
        -- Dismissing the dialog ends an attempt still waiting, so a late browser callback cannot
        -- connect the server; an accepted paste or a finished sign-in still reaches the list.
        if state.status == "waiting" or state.status == "submitting" then
            local effects = stop(state)
            table.insert(
                effects,
                if state.status == "waiting" and state.pasteAccepted then { type = "refresh" } else { type = "cancel" }
            )
            return { status = "disposed" }, effects
        end
        return { status = "disposed" }, if state.status == "completing" then { { type = "sync.clear" } } else {}
    end
    if state.status == "done" then
        return state, {}
    end
    if event.type == "browser.open" then
        if not signin.view(state).showOpenBrowser then
            return state, {}
        end
        return state, { { type = "open-external", url = (state :: any).url } }
    end
    if state.status == "completing" then
        if event.type ~= "sync.succeeded" and event.type ~= "sync.failed" and event.type ~= "sync.timed-out" then
            return state, {}
        end
        if event.syncAttempt ~= state.syncAttempt then
            return state, {}
        end
        if event.type == "sync.succeeded" then
            return { status = "done" }, { { type = "sync.clear" }, { type = "close" } }
        end
        local failure: Failure = if event.type == "sync.failed"
            then { type = "sync-failed", message = event.message }
            else { type = "sync-timed-out" }
        return { status = "failed", url = state.url, issued = state.issued, failure = failure }, { { type = "sync.clear" } }
    end
    if state.status == "failed" then
        if event.type ~= "retry.requested" then
            return state, {}
        end
        if state.failure.type ~= "sync-failed" and state.failure.type ~= "sync-timed-out" then
            return state, {}
        end
        return sync(state)
    end
    if event.type == "polled" then
        return polled(state, event)
    end
    if event.type == "waiting.timed-out" then
        local failure: Failure = if state.unreachableError
            then { type = "auth-failed", message = state.unreachableError }
            else { type = "auth-expired" }
        local effects = stop(state)
        -- An accepted paste is the server's to finish; cancelling it could drop a sign-in that is
        -- about to succeed.
        if not (state.status == "waiting" and state.pasteAccepted) then
            table.insert(effects, { type = "cancel" })
        end
        return { status = "failed", url = state.url, issued = state.issued, failure = failure }, effects
    end
    if state.status == "submitting" then
        if event.type == "submit.succeeded" and event.submitAttempt == state.submitAttempt then
            local pollAttempt = state.issued + 1
            -- Polls at once, so the dialog moves on as soon as the server finishes.
            return {
                status = "waiting",
                url = state.url,
                issued = pollAttempt,
                pollAttempt = pollAttempt,
                value = state.value,
                pasteAccepted = true,
                entryError = nil,
                unreachableError = state.unreachableError,
            },
                { { type = "poll", pollAttempt = pollAttempt, delay = 0 } }
        end
        if event.type == "submit.failed" and event.submitAttempt == state.submitAttempt then
            return {
                status = "waiting",
                url = state.url,
                issued = state.issued,
                pollAttempt = state.pollAttempt,
                value = state.value,
                pasteAccepted = false,
                entryError = { type = "rejected", message = event.message },
                unreachableError = state.unreachableError,
            },
                {}
        end
        return state, {}
    end
    if state.pasteAccepted then
        return state, {}
    end
    if event.type == "paste.changed" then
        if event.value == state.value and state.entryError == nil then
            return state, {}
        end
        local next = table.clone(state)
        next.value = event.value
        next.entryError = nil
        return next, {}
    end
    if event.type ~= "paste.submitted" then
        return state, {}
    end
    local code = state.value:match("^%s*(.-)%s*$") or ""
    if code == "" then
        local next = table.clone(state)
        next.entryError = { type = "required" }
        return next, {}
    end
    local submitAttempt = state.issued + 1
    return {
        status = "submitting",
        url = state.url,
        issued = submitAttempt,
        pollAttempt = state.pollAttempt,
        submitAttempt = submitAttempt,
        value = state.value,
        unreachableError = state.unreachableError,
    },
        { { type = "submit", submitAttempt = submitAttempt, code = code } }
end

return signin
