--!strict
-- The Updater plugin's app half (Desktop's `main.ts`): once per process. The host's updater
-- checks, downloads and verifies on its own schedule; this half adds Check for Updates… to the
-- app menu, after About. A click asks the focused window to check, which reports how it went in
-- the window, as Desktop's beta builds do: windows follow the `updater` contract, whose `checks`
-- grows with each click.

local plugin = require("@opencode/plugin")

return plugin.defineApp(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Contracts = ctx:GetService("Contracts")
    local checks = 0

    local function publish()
        Contracts:Provide("updater", { checks = checks })
    end

    publish()

    ctx:AddMenubarItem({
        menu = "app",
        id = "check",
        label = I18n:T("menu.check"),
        run = function()
            checks += 1
            publish()
        end,
    })
end)
