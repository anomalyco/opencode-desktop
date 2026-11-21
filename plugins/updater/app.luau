--!strict
-- The Updater plugin's app half (Desktop's `main.ts`): once per process. The host's updater
-- checks, downloads and verifies on its own schedule; this half adds Check for Updates… to the
-- app menu, after About, disabled when no updater runs (`main.ts:123`). On beta builds, a click
-- asks the focused window to check with in-app toasts (windows follow the `updater` contract,
-- whose `checks` grows with each click); on non-beta builds, the host intercepts `updater.check`
-- and shows the native OS dialog (`main.ts:69-128`).

local plugin = require("@opencode/plugin")

return plugin.defineApp(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Contracts = ctx:GetService("Contracts")
    local Updater = ctx:GetService("Updater")
    local checks = 0

    local function publish()
        Contracts:Provide("updater", { checks = checks })
    end

    publish()

    local function isEnabled(state: UpdaterState?): boolean
        return state ~= nil and state.phase ~= "disabled"
    end

    local currentEnabled = isEnabled(Updater:State())

    local function register(enabled: boolean)
        ctx:AddMenubarItem({
            menu = "app",
            id = "check",
            label = I18n:T("menu.check"),
            enabled = enabled,
            run = function()
                checks += 1
                publish()
            end,
        })
    end

    register(currentEnabled)

    Updater.StateChanged:Connect(function(state: UpdaterState?)
        local next = isEnabled(state)
        if next ~= currentEnabled then
            currentEnabled = next
            register(next)
        end
    end)
end)
