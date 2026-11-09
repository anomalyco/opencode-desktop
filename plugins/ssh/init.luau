--!strict
-- Desktop's built-in SSH plugin (`packages/gui-plugins/src/ssh`), the window half
-- (`renderer.tsx`, `state.ts`): each host's row in Settings › Servers, Add SSH server
-- (`server.add` and the palette), Connect and Authenticate (`server.row`), and the dialog its
-- prompts show in. The app half (`app.luau`) runs the connections and answers through the `ssh`
-- contract.

local plugin = require("@opencode/plugin")
local dialog = require("@self/dialog")
local row = require("@self/row")

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Toast = ctx:GetService("Toast")
    local Dialog = ctx:GetService("Dialog")
    local Menu = ctx:GetService("Menu")
    local Servers = ctx:GetService("Servers")
    local System = ctx:GetService("System")
    local Contracts = ctx:GetService("Contracts")
    local live = Contracts:Use("ssh")
    local target = ctx:CreateEditor("")
    local name = ctx:CreateEditor("")
    local response = ctx:CreateEditor("")
    local rev = ctx:CreateSignal()

    local function api(): any
        if live:State() ~= "active" then
            return nil
        end
        return live:Get()
    end

    local function item(id: string): any
        local a = api()
        if not a then
            return nil
        end
        for _, s in a.servers do
            if s.config.id == id then
                return s
            end
        end
        return nil
    end

    -- This window's own attempts (Desktop's `window-machine.ts`): which servers it started, the
    -- prompt it answered last, and an attempt whose prompts should open the dialog.
    local started: { [string]: number } = {}
    local answered: { [string]: string } = {}
    local prompting: string? = nil

    local function request(kind: string, payload: any): boolean
        local a = api()
        if not a then
            Toast:Error(I18n:T("common.requestFailed"), I18n:T("error.unavailable"))
            return false
        end
        a.request(kind, payload)
        return true
    end

    local env: any = {
        I18n = I18n,
        Menu = Menu,
        target = target,
        name = name,
        response = response,
        item = item,
        version = function(): string
            return `{live:Generation()}|{rev:Version()}`
        end,
    }

    -- An attempt this window started connects: its row's button spins.
    function env.pending(id: string): boolean
        local it = item(id)
        return started[id] ~= nil and it ~= nil and it.stage == "connecting" and it.attempt == started[id]
    end

    local function start(config: any, replace: boolean)
        if request("start", { id = config.id, target = config.target, name = config.name, replace = replace }) then
            local it = item(config.id)
            started[config.id] = it and it.attempt
            answered[config.id] = nil
        end
        rev:Fire()
    end

    -- The dialog's controller: one dialog at a time.
    local controller: any = { state = nil }
    env.dialog = controller

    function controller.snapshot(): dialog.Snapshot
        local s = controller.state
        local id = s and s.id or ""
        local it = item(id)
        local mine = started[id] ~= nil and it ~= nil and it.attempt == started[id]
        return {
            item = it,
            submitting = false,
            error = mine and it.stage == "failed",
            answeredPrompt = answered[id],
            activeAttempt = mine and (it.stage == "connecting" or it.stage == "authentication"),
        }
    end

    local function dispatch(event: { [string]: any })
        if not controller.state then
            return
        end
        local nextState, effects = dialog.transition(controller.state, event)
        local before = controller.state
        controller.state = nextState
        if before.challenge and nextState.challenge and before.challenge.prompt.id ~= nextState.challenge.prompt.id or (not before.challenge and nextState.challenge) then
            response:SetText("")
        end
        for _, effect in effects do
            if effect.type == "connect" then
                start(effect.config, effect.replace)
            elseif effect.type == "respond" then
                answered[nextState.id] = effect.prompt
                request("respond", { id = nextState.id, prompt = effect.prompt, value = effect.value })
            elseif effect.type == "cancel" then
                request("cancel", { id = nextState.id })
            elseif effect.type == "complete" then
                Dialog:Close()
                local openProject = controller.openProject
                controller.state = nil
                if openProject then
                    Servers:Connect(`ssh:{nextState.id}`)
                end
            end
        end
        rev:Fire()
    end

    function controller.submit()
        dispatch({ type = "target.changed", value = target:Text() })
        dispatch({ type = "name.changed", value = name:Text() })
        dispatch({ type = "submitted", snapshot = controller.snapshot() })
    end

    function controller.respond()
        if controller.state and controller.state.challenge then
            dispatch({ type = "response.changed", prompt = controller.state.challenge.prompt.id, value = response:Text() })
        end
        dispatch({ type = "responded", snapshot = controller.snapshot() })
    end

    local function openDialog(config: any?, promptOnly: boolean, openProject: boolean)
        local id = if config then config.id else System:RandomId()
        target:SetText(if config then config.target else "")
        name:SetText(if config then config.name else "")
        response:SetText("")
        controller.state = dialog.create(id, config, promptOnly)
        controller.openProject = openProject
        Dialog:Open("connect")
        dispatch({ type = "synced", snapshot = controller.snapshot() })
    end

    env.close = function()
        dispatch({ type = "disposed" })
        controller.state = nil
        Dialog:Close()
    end

    -- Connect or Authenticate: a new attempt this window answers; its prompts open the dialog.
    function env.connect(id: string)
        local it = item(id)
        if not it then
            return
        end
        prompting = id
        start(it.config, false)
    end

    function env.forget(id: string)
        request("forget", { id = id })
    end

    -- The app half's state moved: the dialog follows its server, and a row's attempt that asks
    -- something opens one of its own (`dialog.next`).
    live.Changed:Connect(function()
        if controller.state and Dialog:IsOpen("connect") then
            dispatch({ type = "synced", snapshot = controller.snapshot() })
        elseif prompting then
            local it = item(prompting)
            if it and (it.prompt or it.stage == "incompatible") then
                local config = it.config
                prompting = nil
                openDialog(config, true, false)
            elseif not (it and it.stage == "connecting") then
                prompting = nil
            end
        end
        rev:Fire()
    end)
    target.Changed:Connect(function(text)
        dispatch({ type = "target.changed", value = text })
    end)
    name.Changed:Connect(function(text)
        dispatch({ type = "name.changed", value = text })
    end)
    response.Changed:Connect(function(text)
        local s = controller.state
        if s and s.challenge then
            dispatch({ type = "response.changed", prompt = s.challenge.prompt.id, value = text })
        end
    end)
    target.Submitted:Connect(function()
        controller.submit()
    end)
    name.Submitted:Connect(function()
        controller.submit()
    end)
    response.Submitted:Connect(function()
        controller.respond()
    end)

    ctx:AddServerSource({
        id = "ssh",
        title = I18n:T("label"),
        order = 2,
        row = function(v, props)
            return row.render(env, v, props)
        end,
    })

    ctx:AddMenuItem({
        menu = "server.add",
        id = "add",
        title = I18n:T("add"),
        order = 1,
        run = function()
            openDialog(nil, false, false)
        end,
    })

    ctx:AddCommand({
        id = "add",
        title = I18n:T("add"),
        group = I18n:T("command.category.server"),
        keywords = "server ssh remote host",
        run = function()
            openDialog(nil, false, true)
        end,
    })

    ctx:AddMenuItem({
        menu = "server.row",
        id = "connect",
        title = I18n:T("connect"),
        order = 1,
        when = function(key: string): boolean
            local it = item(string.match(key, "^ssh:(.+)$") or "")
            return it ~= nil and it.stage ~= "ready" and it.stage ~= "authentication"
        end,
        enabled = function(key: string): boolean
            return not env.pending(string.match(key, "^ssh:(.+)$") or "")
        end,
        run = function(key: string?)
            local id = key and string.match(key, "^ssh:(.+)$")
            if id then
                env.connect(id)
            end
        end,
    })

    ctx:AddMenuItem({
        menu = "server.row",
        id = "authenticate",
        title = I18n:T("authenticate"),
        order = 1,
        when = function(key: string): boolean
            local it = item(string.match(key, "^ssh:(.+)$") or "")
            return it ~= nil and it.stage == "authentication"
        end,
        enabled = function(key: string): boolean
            return not env.pending(string.match(key, "^ssh:(.+)$") or "")
        end,
        run = function(key: string?)
            local id = key and string.match(key, "^ssh:(.+)$")
            if id then
                env.connect(id)
            end
        end,
    })

    ctx:AddDialog({
        id = "connect",
        title = I18n:T("add"),
        render = function(v)
            return v:Memo("dialog", `{env.version()}|{target:Version()}|{name:Version()}|{response:Version()}`, function()
                return dialog.render(env, v)
            end)
        end,
        onDismiss = function()
            dispatch({ type = "disposed" })
            controller.state = nil
        end,
    })
end)
