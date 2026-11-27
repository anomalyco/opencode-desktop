--!strict
-- Desktop's built-in BTW plugin (`packages/gui-plugins/src/btw`): `/btw <question>` asks the
-- session's model a one-shot side question (`POST /api/session/:id/generate`), without adding
-- to the conversation, and shows it in a tab of its own: the question as a prompt, Working while
-- it runs, then the answer as a reply, or why it failed with Retry. Each tab's chat is kept in
-- the session's store until the tab closes.

local plugin = require("@opencode/plugin")
local machine = require("@self/machine")
local panel = require("@self/panel")

local INSTRUCTIONS =
    "The user is asking a quick side question about the conversation so far. Answer directly and concisely in markdown from what you already know. Do not call any tools and do not take any actions."

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Sessions = ctx:GetService("Sessions")
    local Server = ctx:GetService("Server")
    local Layout = ctx:GetService("Layout")
    local System = ctx:GetService("System")
    local Toast = ctx:GetService("Toast")
    local Store = ctx:GetService("Store")
    local chats = Store:SessionValue("chats", { chats = {} }) :: SessionStoreValue<{ chats: { machine.Saved } }>
    local state = machine.new()
    local handle: Contribution? = nil
    local function refresh()
        if handle then handle:Refresh() end
    end
    -- Ids handed out by this window, so two asks in one millisecond differ.
    local counter = 0

    local function saved(session: string): { machine.Saved }
        local v = chats:Get(session)
        return if type(v) == "table" and type(v.chats) == "table" then v.chats else {}
    end

    local function update(session: string, f: (list: { machine.Saved }) -> ())
        local list = table.clone(saved(session))
        f(list)
        chats:Set(session, { chats = list })
    end

    local dispatch: (session: string, event: machine.Event) -> ()

    -- Tabs whose answer or failure arrived while they were open but not shown (`model.ts`'s
    -- `unread`), by `{session}\n{tab id}`.
    local unread: { [string]: boolean } = {}

    -- A request's answer, failure or timeout. A closed tab or a superseded retry no longer runs
    -- this attempt, so only a current attempt marks its tab.
    local function settle(session: string, id: string, attempt: number, event: machine.Event)
        local chat = state.sessions[session] and state.sessions[session][id]
        local current = chat ~= nil and chat.status.kind == "running" and chat.status.attempt == attempt
        dispatch(session, event)
        if current and Layout:PanelState(id, session) ~= "visible" then
            unread[`{session}\n{id}`] = true
            refresh()
        end
    end

    local function generate(session: string, id: string, attempt: number, question: string)
        local settled = false
        task.delay(machine.GENERATE_TIMEOUT, function()
            if not settled then
                settled = true
                settle(
                    session,
                    id,
                    attempt,
                    { type = "failed", session = session, id = id, attempt = attempt, failure = { reason = "timeout" } }
                )
            end
        end)
        task.spawn(function()
            local res = Server:RequestAsync(
                "POST",
                `/api/session/{session}/generate`,
                { prompt = `{INSTRUCTIONS}\n\n{question}` }
            )
            if settled then
                return
            end
            settled = true
            if not res.ok then
                local message = if res.error then res.error:match("^%s*(.-)%s*$") else nil
                settle(
                    session,
                    id,
                    attempt,
                    {
                        type = "failed",
                        session = session,
                        id = id,
                        attempt = attempt,
                        failure = { reason = "error", message = if message == "" then nil else message },
                    }
                )
                return
            end
            local json = res.json or {}
            local text = if type(json.data) == "table" and type(json.data.text) == "string"
                then json.data.text
                else json.text
            settle(
                session,
                id,
                attempt,
                {
                    type = "succeeded",
                    session = session,
                    id = id,
                    attempt = attempt,
                    text = if type(text) == "string" then text else "",
                    now = System:Now(),
                }
            )
        end)
    end

    dispatch = function(session, event)
        local before = state.sessions[session] and state.sessions[session][event.id]
        local effects = machine.step(state, event)
        if before ~= (state.sessions[session] and state.sessions[session][event.id]) then refresh() end
        for _, effect in effects do
            if effect.type == "persist-question" then
                local info = effect.info
                update(session, function(list)
                    table.insert(
                        list,
                        {
                            id = effect.id,
                            question = info.question,
                            agent = info.agent,
                            model = info.model,
                            created = info.created,
                        }
                    )
                end)
            elseif effect.type == "open-tab" then
                Layout:OpenPanel("main", { instance = effect.id })
            elseif effect.type == "persist-answer" then
                update(session, function(list)
                    for _, c in list do
                        if c.id == effect.id then
                            c.answer = effect.answer
                            c.duration = effect.duration
                            if effect.agent and effect.agent ~= "" then
                                c.agent = effect.agent
                            end
                            if effect.model then
                                c.model = effect.model
                            end
                        end
                    end
                end)
            elseif effect.type == "delete-chat" then
                update(session, function(list)
                    for i = #list, 1, -1 do
                        if list[i].id == effect.id then
                            table.remove(list, i)
                        end
                    end
                end)
            elseif effect.type == "generate" then
                generate(session, effect.id, effect.attempt, effect.question)
            end
        end
    end

    -- The routed session's agent and model, as the app has them.
    local function routed(): (Session?, string?, machine.ChatModel?)
        local s = Sessions:Routed()
        if s == nil or s.id == "" then
            return nil, nil, nil
        end
        local model = if s.modelId and s.modelId ~= ""
            then { providerID = s.providerId or "", id = s.modelId, variant = s.variant }
            else nil
        return s, s.agent, model
    end

    local env: panel.Env = {
        I18n = I18n,
        Models = ctx:GetService("Models"),
        Theme = ctx:GetService("Theme"),
        System = System,
        state = state,
        saved = saved,
        copied = {},
        refresh = refresh,
        unread = function(session, id)
            return unread[`{session}\n{id}`] == true
        end,
        retry = function(session, id)
            local _, agent, model = routed()
            dispatch(
                session,
                {
                    type = "retried",
                    session = session,
                    id = id,
                    now = System:Now(),
                    saved = machine.find(saved(session), id),
                    agent = agent,
                    model = model,
                }
            )
        end,
    }

    ctx:AddCommand({
        id = "ask",
        title = I18n:T("command.title"),
        description = I18n:T("command.description"),
        group = I18n:T("category.session"),
        section = "session",
        slash = { name = "btw", arguments = true },
        hidden = true,
        -- Offered only while a session is open in a desktop-width window.
        requiresSession = true,
        enabled = function()
            return not Layout:Narrow()
        end,
        run = function(input)
            local question = (input or ""):match("^%s*(.-)%s*$") or ""
            if question == "" then
                Toast:Info(I18n:T("question.required"))
                return nil
            end
            local session, agent, model = routed()
            if session == nil then
                return nil
            end
            local now = System:Now()
            counter += 1
            local id = string.format("%x%04x", now, counter)
            dispatch(
                session.id,
                {
                    type = "asked",
                    session = session.id,
                    id = id,
                    info = { question = question, created = now, agent = agent, model = model },
                }
            )
            return nil
        end,
    })

    -- One tab per question, titled by it.
    handle = ctx:AddPanel({
        id = "main",
        title = I18n:T("tab.title"),
        icon = "comment",
        transient = true,
        list = function(session, open)
            local stored = saved(session)
            local tabs: { PanelTab } = {}
            for _, id in open do
                local question = machine.tab(state, stored, session, id)
                if question then
                    table.insert(tabs, { instance = id, title = question })
                end
            end
            return tabs
        end,
        render = function(view, props)
            -- Shown on screen, the tab's answer is read (`renderer.tsx`: `btw.seen` while the panel
            -- is visible).
            local id = props.instance or ""
            local key = `{props.sessionId}\n{id}`
            if unread[key] and Layout:PanelState(id, props.sessionId) == "visible" then
                unread[key] = nil
                task.defer(refresh)
            end
            return panel.render(env, view, props)
        end,
        label = function(view, props)
            return panel.label(env, view, props)
        end,
        onClose = function(props)
            if props.instance then
                env.copied[props.instance] = nil
                unread[`{props.sessionId}\n{props.instance}`] = nil
                dispatch(props.sessionId, { type = "removed", session = props.sessionId, id = props.instance })
            end
        end,
    })
end)
