--!strict
-- The Copy link button (Desktop's `copy-machine.ts`). Every phase but `idle` names the link it
-- copied, so a code that replaces the link never shows "Copied" or the copy error for it.
-- `attempt` is the last copy handed out.

local copy = {}

-- Seconds "Copied" shows after a copy.
copy.FEEDBACK = 2

export type State =
    { status: "idle", attempt: number }
    | { status: "copying", attempt: number, url: string }
    | { status: "copied", attempt: number, url: string }
    | { status: "failed", attempt: number, url: string }
    | { status: "disposed" }

export type Event = { type: "copy.requested", url: string } | { type: "copy.succeeded", attempt: number } | { type: "copy.failed", attempt: number } | { type: "feedback.ended", attempt: number } | { type: "reset" } | { type: "disposed" }
export type Effect = { type: "cancel-timer" } | { type: "write", attempt: number, url: string } | { type: "schedule-feedback", attempt: number, delay: number }

function copy.create(): State
    return { status = "idle", attempt = 0 }
end

function copy.transition(state: State, e: Event): (State, { Effect })
    if state.status == "disposed" then
        return state, {}
    end
    local s = state :: any
    if e.type == "disposed" then
        return { status = "disposed" }, { { type = "cancel-timer" } }
    elseif e.type == "copy.requested" then
        if s.status == "copying" and s.url == e.url then
            return state, {}
        end
        local attempt = s.attempt + 1
        return { status = "copying", attempt = attempt, url = e.url }, { { type = "cancel-timer" }, { type = "write", attempt = attempt, url = e.url } }
    elseif e.type == "copy.succeeded" or e.type == "copy.failed" then
        if s.status ~= "copying" or s.attempt ~= e.attempt then
            return state, {}
        end
        if e.type == "copy.failed" then
            return { status = "failed", attempt = s.attempt, url = s.url }, { { type = "cancel-timer" } }
        end
        return { status = "copied", attempt = s.attempt, url = s.url }, { { type = "cancel-timer" }, { type = "schedule-feedback", attempt = s.attempt, delay = copy.FEEDBACK } }
    elseif e.type == "feedback.ended" then
        if s.status ~= "copied" or s.attempt ~= e.attempt then
            return state, {}
        end
        return { status = "idle", attempt = s.attempt }, {}
    end
    if s.status == "idle" then
        return state, {}
    end
    return { status = "idle", attempt = s.attempt }, { { type = "cancel-timer" } }
end

-- What the copy button shows for the link on screen now (`viewCopy`).
function copy.view(state: State, url: string?)
    local s = state :: any
    local phase = if s.status ~= "idle" and s.status ~= "disposed" and s.url == url then s.status else "idle"
    if phase == "copied" then
        return { icon = "check", label = "common.copied", disabled = false, error = false }
    end
    return { icon = "copy", label = "copyLink", disabled = phase == "copying", error = phase == "failed" }
end

return copy
