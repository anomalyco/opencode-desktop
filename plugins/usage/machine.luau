--!strict
-- Usage dialog state machine (`usage/machine.ts`): `loading`, `refreshing` (shows cached limits
-- while the fresh read runs), `ready`, `signed-out`, and `failed`. Every in-flight read and its
-- 15 s timeout carry an attempt number so stale answers are dropped.

local limits = require("./limits")

local machine = {}

-- `USAGE_LOAD_TIMEOUT_MS`: seconds before a pending limits read times out.
machine.TIMEOUT_S = 15

export type Load =
    { status: "loading", attempt: number }
    | { status: "refreshing", attempt: number, info: limits.Info }
    | { status: "ready", info: limits.Info }
    | { status: "signed-out" }
    | { status: "failed" }

export type OpenState = {
    directory: string,
    model: limits.CurrentModel?,
    credits: boolean,
    load: Load,
    issued: number,
}

export type Event =
    { type: "loaded", attempt: number, info: limits.Info? }
    | { type: "failed", attempt: number }
    | { type: "timed-out", attempt: number }
    | { type: "retry" }

export type Effect = {
    type: "sync",
    attempt: number,
}

local function beginLoad(open: OpenState, cached: limits.Info?): Effect
    local attempt = open.issued + 1
    open.issued = attempt
    if cached ~= nil then
        open.load = { status = "refreshing", attempt = attempt, info = cached }
    else
        open.load = { status = "loading", attempt = attempt }
    end
    return { type = "sync", attempt = attempt }
end

function machine.open(
    directory: string,
    model: limits.CurrentModel?,
    credits: boolean,
    cached: limits.Info?
): (OpenState, Effect)
    local state: OpenState = {
        directory = directory,
        model = model,
        credits = credits,
        load = { status = "failed" },
        issued = 0,
    }
    local effect = beginLoad(state, cached)
    return state, effect
end

-- `transitionUsageDialog`.
function machine.step(open: OpenState, event: Event): Effect?
    local waiting: number? = if open.load.status == "loading" or open.load.status == "refreshing"
        then open.load.attempt
        else nil

    if event.type == "retry" then
        if open.load.status == "failed" then
            return beginLoad(open, nil)
        end
        return nil
    end

    if waiting ~= event.attempt then
        return nil
    end

    if event.type == "loaded" then
        if event.info ~= nil then
            open.load = { status = "ready", info = event.info }
        else
            open.load = { status = "signed-out" }
        end
        return nil
    end

    if event.type == "failed" or event.type == "timed-out" then
        open.load = { status = "failed" }
        return nil
    end

    return nil
end

return machine
