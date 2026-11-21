--!strict
-- The browser's app half: once per process, whatever the number of windows. It keeps the pages
-- the browser showed (Desktop's `Store.global(History)`: one list for every session and server,
-- kept on this device) and is its only writer, so windows that record at once don't overwrite
-- each other. Windows read the list from the store and change it through the `browser.history`
-- contract.

local plugin = require("@opencode/plugin")
local history = require("./history")

return plugin.defineApp(function(ctx)
    local Store = ctx:GetService("Store")
    local Contracts = ctx:GetService("Contracts")
    local stored = Store:Value("history", { visits = {} })

    local function visits(): { history.Visit }
        local value = stored:Get()
        return if type(value) == "table" and type(value.visits) == "table" then value.visits else {}
    end

    Contracts:Provide("browser.history", {
        -- A page a window showed, once loaded: first in the list, replacing an earlier visit of
        -- its address.
        remember = function(url: string, title: string, icon: string?)
            if history.recordable(url) then
                stored:Set({ visits = history.remember(visits(), { url = url, title = title, icon = icon }) })
            end
        end,
        forget = function(url: string)
            stored:Set({ visits = history.forget(visits(), url) })
        end,
        clear = function()
            stored:Set({ visits = {} })
        end,
    })
end)
