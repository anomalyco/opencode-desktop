--!strict
-- Desktop's built-in Context plugin (`packages/gui-plugins/src/context`): the session header's
-- context ring (its `session.header` slot, order 10) with the session's tokens and spend, which
-- opens the Context tab; and that tab, with the session's token and cost figures, its system
-- prompt and its raw messages, which it exports.

local plugin = require("@opencode/plugin")
local indicator = require("@self/indicator")
local tab = require("@self/tab")

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local state: tab.State = { open = {}, export = { kind = "idle" }, parsed = {} }
    local env = {
        Sessions = ctx:GetService("Sessions"),
        Models = ctx:GetService("Models"),
        Server = ctx:GetService("Server"),
        I18n = I18n,
        Theme = ctx:GetService("Theme"),
        Toast = ctx:GetService("Toast"),
        Workspace = ctx:GetService("Workspace"),
        Layout = ctx:GetService("Layout"),
        Popover = ctx:GetService("Popover"),
        state = state,
    }

    -- The raw messages shown open survive a reload.
    ctx:BindHotState(function()
        return state.open
    end, function(saved)
        if type(saved) == "table" then
            state.open = saved
        end
    end)

    ctx:AddSlot({
        id = "usage",
        at = "session.header",
        order = 10,
        render = function(view, props)
            return indicator.header(env, view, props)
        end,
    })

    ctx:AddPopover({
        id = "usage",
        title = I18n:T("usage.view"),
        render = function(view, props)
            return indicator.menu(env, view, props)
        end,
    })

    -- Opened from its header button, so it has no New tab card (Desktop gives it no menu item).
    ctx:AddPanel({
        id = "main",
        title = I18n:T("tab.title"),
        description = I18n:T("mobile.title"),
        icon = "status",
        launcher = false,
        first = true,
        fallback = true,
        render = function(view, props)
            return tab.panel(env, view, props)
        end,
        label = function(view, props)
            return indicator.label(env, view, props)
        end,
    })
end)
