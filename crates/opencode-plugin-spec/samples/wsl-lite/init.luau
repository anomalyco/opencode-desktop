--!strict
-- The window half: one per window. It adds the Add WSL server dialog and a settings section, and
-- reaches the app half through its contract.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")

type WslApi = {
    refresh: () -> (),
    start: (name: string) -> (),
}

return plugin.defineWindow(function(ctx)
    local Dialog = ctx:GetService("Dialog")
    local Contracts = ctx:GetService("Contracts")
    local Store = ctx:GetService("Store")
    local Servers = ctx:GetService("Servers")
    local App = ctx:GetService("App")

    local wsl = Contracts:Use("wsl-lite")
    local autoStart = Store:Value("auto-start", false)
    local distro = ctx:CreateEditor("")

    local function api(): WslApi?
        if wsl:State() ~= "active" then
            return nil
        end
        return wsl:Get() :: WslApi
    end

    local function add()
        local name = distro:Text()
        local target = api()
        if target and #name > 0 then
            target.start(name)
            distro:SetText("")
            Dialog:Close()
        end
    end

    ctx:AddMenuItem({
        menu = "server.add",
        id = "add",
        title = "Add WSL server",
        run = function()
            Dialog:Open("add")
        end,
    })

    ctx:AddMenuItem({
        menu = "server.row",
        id = "restart",
        title = "Restart",
        when = function(key)
            return key:sub(1, #"wsl-lite:") == "wsl-lite:"
        end,
        run = function(key)
            local target = api()
            if target and key then
                target.start(key:sub(#"wsl-lite:" + 1))
            end
        end,
    })

    ctx:AddDialog({
        id = "add",
        title = "Add WSL server",
        size = "normal",
        render = function(view)
            return view:Memo("add", distro:Version(), function()
                return ui.col({ gap = 12, w = "full" }, {
                    ui.text(
                        "Starts OpenCode inside a WSL distro and adds it to your servers.",
                        { color = "text-weak", wrap = true }
                    ),
                    ui.field({
                        key = "wsl-distro",
                        editor = distro,
                        placeholder = "Ubuntu",
                        onSubmit = view:Handler(add),
                    }),
                    ui.row({ w = "full", gap = 8, justify = "end" }, {
                        ui.button({
                            text = "Cancel",
                            variant = "ghost",
                            onPress = view:Handler(function()
                                Dialog:Close()
                            end),
                        }),
                        ui.button({
                            text = "Add",
                            variant = "primary",
                            disabled = #distro:Text() == 0,
                            onPress = view:Handler(add),
                        }),
                    }),
                })
            end)
        end,
    })

    ctx:AddSettingsPage({
        id = "wsl",
        title = "WSL",
        page = "servers",
        entries = {
            {
                id = "wsl-auto-start",
                title = "Start WSL servers with the app",
                keywords = "wsl linux distro server start",
            },
        },
        render = function(view)
            local count = 0
            for _, server in Servers:List() do
                if server.label == "WSL" then
                    count += 1
                end
            end
            return ui.settingsList({ title = `WSL ({count} distros)` }, {
                ui.settingsRow(
                    {
                        id = "wsl-auto-start",
                        title = "Start WSL servers with the app",
                        description = if App.Platform == "windows" then nil else "Only on Windows",
                    },
                    ui.switch({
                        checked = autoStart:Get(),
                        disabled = App.Platform ~= "windows",
                        onChange = view:Handler(function(event)
                            autoStart:Set(event.checked == true)
                        end),
                    })
                ),
            })
        end,
    })
end)
