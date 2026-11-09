--!strict
-- The latest turn's numbers, as the TUI's last assistant footer shows them (`bar.tsx`,
-- `util/session-turn.ts`, `util/duration.ts`): how long the turn took, and its tokens per second.

local turn = {}

export type Turn = {
    -- Compact elapsed time; nil while the step runs.
    duration: string?,
    tps: number?,
}

local function truthy(v: any): boolean
    return v ~= nil and v ~= false and v ~= ""
end

local function time(m: any, field: string): number?
    local t = if type(m.time) == "table" then m.time[field] else nil
    return if type(t) == "number" then math.floor(t) else nil
end

-- The turn's prompt before `stop` (exclusive): the nearest input in legacy sessions, otherwise the
-- first input after the previous idle marker.
local function inputIndex(messages: { any }, stop: number, legacy: boolean): number?
    local input: number? = nil
    for i = stop - 1, 1, -1 do
        local ty = messages[i].type
        if ty == "idle" then
            return input
        elseif ty == "user" or ty == "synthetic" then
            if legacy then
                return i
            end
            input = i
        end
    end
    return input
end

-- Milliseconds from the prompt that started the turn until message `index` completed, or 0 while
-- it runs.
local function duration(messages: { any }, index: number, legacy: boolean): number
    local message = messages[index]
    local completed = time(message, "completed")
    if completed == nil then
        return 0
    end
    local i = inputIndex(messages, index, legacy)
    local input = if i then time(messages[i], "created") else nil
    return math.max(completed - (input or time(message, "created") or completed), 0)
end

-- Output and reasoning tokens per second across the turn's steps up to message `index`, or nil
-- until every step has streamed.
local function tokensPerSecond(messages: { any }, index: number, legacy: boolean): number?
    local i = inputIndex(messages, index + 1, legacy)
    local start = if i then i + 1 else 1
    local output, total, steps, streamed = 0, 0, 0, 0
    for k = start, index do
        local s = messages[k]
        if s.type == "assistant" then
            steps += 1
            local a, b = time(s, "streamed"), time(s, "created")
            if a and b then
                streamed += 1
                total += math.max(a - b, 0)
            end
            local tokens = if type(s.tokens) == "table" then s.tokens else {}
            output += (if type(tokens.output) == "number" then tokens.output else 0) + (if type(tokens.reasoning)
                    == "number"
                then tokens.reasoning
                else 0)
        end
    end
    if steps == 0 or streamed ~= steps or output <= 0 or total <= 0 then
        return nil
    end
    -- Aggregate before dividing so each step is weighted by its provider-active duration.
    return output / (total / 1000)
end

-- Compact elapsed time: `850ms`, `12.4s`, `3m 5s`, `2h 10m` or `1d 4h`.
function turn.compact(ms: number): string
    if ms < 1000 then
        return `{ms}ms`
    elseif ms < 60000 then
        return string.format("%.1fs", ms / 1000)
    elseif ms < 3600000 then
        return `{ms // 60000}m {ms % 60000 // 1000}s`
    elseif ms < 86400000 then
        return `{ms // 3600000}h {ms % 3600000 // 60000}m`
    end
    return `{ms // 86400000}d {ms % 86400000 // 3600000}h`
end

-- The numbers of the latest assistant message that ended its turn, failed or is retrying; nil
-- when there is none. `messages` are in the session's order.
function turn.latest(messages: { any }): Turn?
    local index: number? = nil
    local legacy = true
    for i, m in messages do
        if m.type == "idle" then
            legacy = false
        end
        local finish = m.finish
        local ended = type(finish) == "string" and finish ~= "" and finish ~= "tool-calls" and finish ~= "unknown"
        if m.type == "assistant" and (truthy(m.retry) or truthy(m.error) or ended) then
            index = i
        end
    end
    if index == nil then
        return nil
    end
    -- The TUI hides a zero duration, which means the step has not completed.
    local elapsed = duration(messages, index, legacy)
    return {
        duration = if elapsed > 0 then turn.compact(elapsed) else nil,
        tps = tokensPerSecond(messages, index, legacy),
    }
end

return turn
