--!strict
-- The side questions of every session (`machine.ts`): one chat per tab, each asked once and
-- retried on demand. `step` takes an event and returns the effects to run against the server and
-- the session store; chats this window never touched are read from the store.

local machine = {}

-- Longest one answer may take before it fails with Retry; a stalled connection never settles.
machine.GENERATE_TIMEOUT = 300

export type ChatModel = { providerID: string, id: string, variant: string? }

-- What a chat was asked with: its question, when, and the agent and model then.
export type Info = { question: string, created: number?, agent: string?, model: ChatModel? }

-- One chat as the session store keeps it (Desktop's `Chats` schema).
export type Saved = {
    id: string,
    question: string,
    answer: string?,
    agent: string?,
    model: ChatModel?,
    created: number?,
    duration: number?,
}

-- Why a chat has no answer. `interrupted` is a stored chat without an answer: a reload, closed
-- session or disposed plugin abandoned its request. `empty` is a reply without visible text.
export type Failure =
    { reason: "interrupted" }
    | { reason: "empty" }
    | { reason: "timeout" }
    | { reason: "error", message: string? }

export type Status =
    { kind: "running", attempt: number, started: number }
    | { kind: "answered", answer: string, duration: number }
    | { kind: "failed", failure: Failure }

-- A chat this window asked or retried.
export type Chat = { info: Info, status: Status }

export type State = {
    -- The last attempt id handed out.
    issued: number,
    -- Chats by session, then chat id.
    sessions: { [string]: { [string]: Chat } },
}

export type Event =
    { type: "asked", session: string, id: string, info: Info }
    | { type: "retried", session: string, id: string, now: number, saved: Saved?, agent: string?, model: ChatModel? }
    | { type: "succeeded", session: string, id: string, attempt: number, text: string, now: number }
    | { type: "failed", session: string, id: string, attempt: number, failure: Failure }
    | { type: "removed", session: string, id: string }

export type Effect =
    { type: "persist-question", id: string, info: Info }
    | { type: "open-tab", id: string }
    | { type: "generate", id: string, attempt: number, question: string }
    | { type: "persist-answer", id: string, answer: string, duration: number, agent: string?, model: ChatModel? }
    | { type: "delete-chat", id: string }

-- What a chat's panel shows.
export type Phase =
    { kind: "missing" }
    | { kind: "running" }
    | { kind: "failed", failure: Failure }
    | { kind: "answered", answer: string, duration: number? }

function machine.new(): State
    return { issued = 0, sessions = {} }
end

local function live(state: State, session: string, id: string): Chat?
    local chats = state.sessions[session]
    return if chats then chats[id] else nil
end

local function put(state: State, session: string, id: string, chat: Chat)
    local chats = state.sessions[session] or {}
    chats[id] = chat
    state.sessions[session] = chats
end

local function trim(s: string): string
    return (s:match("^%s*(.-)%s*$") or "")
end

-- The running chat of `attempt`, if it still is.
local function running(state: State, session: string, id: string, attempt: number): (Info?, number)
    local c = live(state, session, id)
    if c and c.status.kind == "running" and c.status.attempt == attempt then
        return c.info, c.status.started
    end
    return nil, 0
end

function machine.step(state: State, event: Event): { Effect }
    if event.type == "asked" then
        local question = trim(event.info.question)
        if question == "" or live(state, event.session, event.id) then
            return {}
        end
        state.issued += 1
        local attempt = state.issued
        local info = table.clone(event.info)
        info.question = question
        put(
            state,
            event.session,
            event.id,
            { info = info, status = { kind = "running", attempt = attempt, started = info.created or 0 } }
        )
        return {
            { type = "persist-question", id = event.id, info = info },
            { type = "open-tab", id = event.id },
            { type = "generate", id = event.id, attempt = attempt, question = question },
        }
    elseif event.type == "retried" then
        -- A stored chat this window never asked (one restored after a reload) is retryable too.
        local c = live(state, event.session, event.id)
        local info: Info
        if c then
            if c.status.kind ~= "failed" then
                return {}
            end
            info = table.clone(c.info)
        elseif event.saved and trim(event.saved.answer or "") == "" then
            local s = event.saved
            info = { question = s.question, created = s.created, agent = s.agent, model = s.model }
        else
            return {}
        end
        state.issued += 1
        local attempt = state.issued
        if event.agent and event.agent ~= "" then
            info.agent = event.agent
        end
        info.model = event.model or info.model
        put(
            state,
            event.session,
            event.id,
            { info = info, status = { kind = "running", attempt = attempt, started = event.now } }
        )
        return { { type = "generate", id = event.id, attempt = attempt, question = info.question } }
    elseif event.type == "succeeded" then
        local info, started = running(state, event.session, event.id, event.attempt)
        if info == nil then
            return {}
        end
        local answer = trim(event.text)
        -- A reply without visible text is a failure the user can retry, not an empty answer.
        if answer == "" then
            put(
                state,
                event.session,
                event.id,
                { info = info, status = { kind = "failed", failure = { reason = "empty" } } }
            )
            return {}
        end
        local duration = math.max(event.now - started, 0)
        put(
            state,
            event.session,
            event.id,
            { info = info, status = { kind = "answered", answer = answer, duration = duration } }
        )
        return {
            {
                type = "persist-answer",
                id = event.id,
                answer = answer,
                duration = duration,
                agent = info.agent,
                model = info.model,
            },
        }
    elseif event.type == "failed" then
        local info = running(state, event.session, event.id, event.attempt)
        if info then
            put(state, event.session, event.id, { info = info, status = { kind = "failed", failure = event.failure } })
        end
        return {}
    end
    -- A request still running answers into no chat (its attempt is gone), so it needs no abort.
    local chats = state.sessions[event.session]
    if chats then
        chats[event.id] = nil
        if next(chats) == nil then
            state.sessions[event.session] = nil
        end
    end
    return { { type = "delete-chat", id = event.id } }
end

local function find(saved: { Saved }, id: string): Saved?
    for _, s in saved do
        if s.id == id then
            return s
        end
    end
    return nil
end

machine.find = find

-- The question `id`'s tab shows: the live chat's, else the stored one's; nil when it has none.
function machine.tab(state: State, saved: { Saved }, session: string, id: string): string?
    local c = live(state, session, id)
    if c then
        return c.info.question
    end
    local s = find(saved, id)
    return if s then s.question else nil
end

-- What `id`'s panel shows, with what the chat was asked with.
function machine.view(state: State, saved: { Saved }, session: string, id: string): (Phase, Info?)
    local c = live(state, session, id)
    if c then
        local st = c.status
        if st.kind == "running" then
            return { kind = "running" }, c.info
        elseif st.kind == "failed" then
            return { kind = "failed", failure = st.failure }, c.info
        end
        return { kind = "answered", answer = st.answer, duration = st.duration }, c.info
    end
    local s = find(saved, id)
    if s == nil then
        return { kind = "missing" }, nil
    end
    local info: Info = { question = s.question, created = s.created, agent = s.agent, model = s.model }
    if s.answer == nil then
        return { kind = "failed", failure = { reason = "interrupted" } }, info
    end
    local answer = trim(s.answer)
    -- Builds before this machine stored a blank reply as the answer.
    if answer == "" then
        return { kind = "failed", failure = { reason = "empty" } }, info
    end
    return { kind = "answered", answer = answer, duration = s.duration }, info
end

return machine
