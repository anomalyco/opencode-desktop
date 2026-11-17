--!strict
-- Desktop's built-in WSL plugin (`packages/gui-plugins/src/wsl`), the window half
-- (`renderer.tsx`): each WSL server's row in Settings › Servers, Add WSL server (`server.add`),
-- Retry start (`server.row`) and the add dialog. The app half (`app.luau`) runs the servers and
-- answers through the `wsl` contract.

local plugin = require("@opencode/plugin")
local dialog = require("@self/dialog")
local view = require("@self/view")
local row = require("@self/row")
local model = require("@self/model")

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Toast = ctx:GetService("Toast")
    local Dialog = ctx:GetService("Dialog")
    local Menu = ctx:GetService("Menu")
    local App = ctx:GetService("App")
    local Contracts = ctx:GetService("Contracts")
    local wsl = Contracts:Use("wsl")
    local search = ctx:CreateEditor("")
    local rev = ctx:CreateSignal()

    -- The app half's `wsl` contract while it answers.
    local function api(): any
        if wsl:State() ~= "active" then
            return nil
        end
        return wsl:Get()
    end

    local env: any = {
        I18n = I18n,
        Toast = Toast,
        Dialog = Dialog,
        Menu = Menu,
        menuTrigger = true,
        api = api,
        search = search,
        changed = function()
            rev:Fire()
        end,
        version = function(): string
            return `{wsl:Generation()}|{rev:Version()}`
        end,
    }
    env.controller = dialog.controller(env)
    env.close = function()
        env.controller.dispose()
        Dialog:Close()
    end

    -- What a row can do follows the app half's state; a request only reports its failure.
    local pending: { [string]: boolean } = {}
    env.run = function(kind: string, arg: string)
        local a = api()
        if not a then
            Toast:Error(I18n:T("common.requestFailed"), I18n:T("error.unavailable"))
            return
        end
        pending[tostring(a.request(kind, arg))] = true
    end

    local function settleRows()
        local a = api()
        if not a then
            return
        end
        for id in pending do
            local result = a.results[id]
            if result then
                pending[id] = nil
                if result.error then
                    Toast:Error(I18n:T("common.requestFailed"), result.error)
                end
            end
        end
    end

    local available = false
    wsl.Changed:Connect(function()
        local now = wsl:State() == "active"
        if now ~= available then
            available = now
            env.controller.availability(now)
        end
        settleRows()
        if Dialog:IsOpen("add") then
            env.controller.refresh()
        end
    end)
    search.Changed:Connect(function()
        env.controller.refresh()
    end)

    ctx:AddServerSource({
        id = "wsl",
        title = I18n:T("server.label"),
        order = 1,
        row = function(v, props)
            return row.render(env, v, props)
        end,
    })

    ctx:AddMenuItem({
        menu = "server.add",
        id = "add",
        title = I18n:T("server.add"),
        order = 2,
        run = function()
            search:SetText("")
            env.controller.open()
            Dialog:Open("add")
        end,
    })

    ctx:AddMenuItem({
        menu = "server.row",
        id = "retry",
        title = I18n:T("server.retryStart"),
        when = function(key: string): boolean
            local distro = string.match(key, "^wsl:(.+)$")
            return distro ~= nil and model.row(api() and api().state, distro).retry
        end,
        run = function(key: string?)
            local distro = key and string.match(key, "^wsl:(.+)$")
            if distro then
                env.run("startServer", `wsl:{distro}`)
            end
        end,
    })

    ctx:AddDialog({
        id = "add",
        title = I18n:T("server.add"),
        render = function(v)
            return v:Memo("dialog", `{env.version()}|{search:Version()}`, function()
                return view.dialog(env, v)
            end)
        end,
        onDismiss = function()
            env.controller.dispose()
        end,
    })
end)
