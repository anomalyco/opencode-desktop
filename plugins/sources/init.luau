--!strict
-- Desktop's built-in Sources plugin (`packages/gui-plugins/src/sources`): the Sources
-- experiment's side panel, one transient tab per turn, keyed by the user message that opened it.
-- The timeline's "N sources" chip opens it with a link only this plugin takes
-- (`opencode-sources:<message id>`). Turning the experiment off stops listing the tabs, which
-- closes them.

local plugin = require("@opencode/plugin")
local panel = require("@self/panel")

-- The links the timeline's chip opens: the turn's user message id after it.
local LINK = "opencode-sources:"

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Diagnostics = ctx:GetService("Diagnostics")
    local Layout = ctx:GetService("Layout")
    local handle: Contribution? = nil
    local env: panel.Env = {
        Sessions = ctx:GetService("Sessions"),
        Server = ctx:GetService("Server"),
        Links = ctx:GetService("Links"),
        I18n = I18n,
        Theme = ctx:GetService("Theme"),
        Images = ctx:GetService("Images"),
        Diagnostics = Diagnostics,
        state = { expand = {}, parsed = {} },
        refresh = function() if handle then handle:Refresh() end end,
    }

    local function enabled(): boolean
        return Diagnostics:Get("sources.enabled") == "on"
    end

    ctx:AddLinkHandler({
        id = "turn",
        match = function(link)
            return enabled() and link.href:sub(1, #LINK) == LINK
        end,
        open = function(link)
            Layout:OpenPanel("turn", { instance = link.href:sub(#LINK + 1) })
        end,
    })

    -- One tab per turn while the experiment is on, titled "Sources · N" (`tab.count`).
    handle = ctx:AddPanel({
        id = "turn",
        title = I18n:T("title"),
        icon = "globe",
        transient = true,
        list = function(_session, open)
            if not enabled() then
                return {}
            end
            local tabs: { PanelTab } = {}
            for _, id in open do
                table.insert(tabs, { instance = id, title = I18n:T("title") })
            end
            return tabs
        end,
        render = function(view, props)
            if not enabled() then
                return nil
            end
            return panel.render(env, view, props)
        end,
        label = function(view, props)
            return panel.label(env, view, props)
        end,
    })
end)
