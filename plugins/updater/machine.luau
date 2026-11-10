--!strict
-- The update action this window asked for (`window-machine.ts`). The host's updater state says
-- what the app is doing; this says what the window is doing about it, so a second request joins
-- the first instead of showing a second "Checking…" toast and result.
-- - `idle`: nothing asked; `issued` is the last attempt handed out.
-- - `checking`: a check runs, with its toast, until it reports what it found.
-- - `installing`: the install is being handed to the host.
-- - `disposed`: the window let the updater go.

local machine = {}

export type Action = "check" | "install"

export type State =
    { status: "idle", issued: number }
    | { status: "checking", attempt: number }
    | { status: "installing", attempt: number }
    | { status: "disposed" }

export type Event =
    -- The user asked from the menu, a command, Settings or the titlebar. `available` is false
    -- while no updater runs.
    { type: "action.requested", action: Action, available: boolean }
    -- The action finished and answered the user.
    | { type: "action.settled", attempt: number }
    | { type: "disposed" }

export type Effect = { type: "run", action: Action, attempt: number } | { type: "toast-failed", message: string? }

machine.initial = { status = "idle", issued = 0 } :: State

function machine.transition(state: State, event: Event): (State, { Effect })
    if state.status == "disposed" then
        return state, {}
    end
    if event.type == "disposed" then
        return { status = "disposed" }, {}
    end
    if event.type == "action.requested" then
        if state.status == "installing" then
            return state, {}
        end
        -- No updater runs here: nothing can check or install.
        if not event.available then
            return state, if state.status == "idle" then { { type = "toast-failed" } :: Effect } else {}
        end
        if state.status == "checking" and event.action == "check" then
            return state, {}
        end
        -- An install asked for during a check takes over; the check's answer settles nothing.
        local attempt = (if state.status == "idle" then state.issued else (state :: any).attempt) + 1
        local next: State = if event.action == "check" then { status = "checking", attempt = attempt } else { status = "installing", attempt = attempt }
        return next, { { type = "run", action = event.action, attempt = attempt } }
    end
    if state.status == "idle" or event.attempt ~= (state :: any).attempt then
        return state, {}
    end
    return { status = "idle", issued = (state :: any).attempt }, {}
end

return machine
