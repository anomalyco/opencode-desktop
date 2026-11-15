--!strict
-- Announcing an update the host's own checks found (launch, then every ten minutes): the Update
-- Ready or Update Available toast, once per version. The version announced last is kept in the
-- `updateNotice` store, which every window shares and which outlives the run, so Later (or the
-- toast's ×) means that version never toasts again; a newer one does. A manual check announces
-- what it found itself (`announced`).
-- - `quiet`: nothing to announce.
-- - `pending`: an update to announce once a window of this app has focus.
-- - `disposed`: the window let the updater go.

local notice = {}

export type Kind = "ready" | "download-required"

export type State = { status: "quiet" } | { status: "pending", version: string, kind: Kind } | { status: "disposed" }

export type Event =
    -- The host's updater moved. `notified` is the store's version announced last.
    { type: "updater", phase: string?, version: string?, focused: boolean, notified: string? }
    | { type: "focused", notified: string? }
    -- A manual check showed this version's toast.
    | { type: "announced", version: string }
    | { type: "disposed" }

export type Effect = { type: "show", version: string, kind: Kind } | { type: "mark", version: string }

notice.initial = { status = "quiet" } :: State

local QUIET: State = { status = "quiet" }

local function announce(version: string, kind: Kind): (State, { Effect })
    return QUIET, { { type = "show", version = version, kind = kind }, { type = "mark", version = version } }
end

function notice.transition(state: State, event: Event): (State, { Effect })
    if state.status == "disposed" then
        return state, {}
    end
    if event.type == "disposed" then
        return { status = "disposed" }, {}
    end
    if event.type == "announced" then
        return QUIET, { { type = "mark", version = event.version } }
    end
    if event.type == "focused" then
        if state.status ~= "pending" then
            return state, {}
        end
        -- Another window may have announced it meanwhile.
        if state.version == event.notified then
            return QUIET, {}
        end
        return announce(state.version, state.kind)
    end
    local phase, version = event.phase, event.version
    if (phase ~= "ready" and phase ~= "download-required") or not version then
        return QUIET, {}
    end
    if version == event.notified then
        return QUIET, {}
    end
    local kind: Kind = if phase == "ready" then "ready" else "download-required"
    if not event.focused then
        return { status = "pending", version = version, kind = kind }, {}
    end
    return announce(version, kind)
end

return notice
