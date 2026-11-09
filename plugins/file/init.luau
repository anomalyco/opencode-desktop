--!strict
-- Desktop's built-in file plugin (`packages/gui-plugins/src/file`): a session's file tabs in its
-- side region (`file:open`, the Open file launcher, and `file://<path>`, one per open file), Open
-- file (Ctrl+Alt+P, ^?P on macOS) and the + menu's Open file row, and the narrow window's Files
-- view. The tree, the files and their code come from the host's file engine (`Workspace`); the
-- change letters from the review's `review.changes`.

local plugin = require("@opencode/plugin")
local panel = require("@self/panel")

-- The panel's id: its tabs key as `file:<instance>`, as Desktop's.
local PANEL = "main"

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Sessions = ctx:GetService("Sessions")
    local Layout = ctx:GetService("Layout")
    local App = ctx:GetService("App")
    local Workspace = ctx:GetService("Workspace")
    local Contracts = ctx:GetService("Contracts")
    local env: panel.Env = {
        I18n = I18n,
        Layout = Layout,
        Workspace = Workspace,
        changes = Contracts:Use("review.changes"),
        filter = ctx:CreateEditor(""),
    }

    -- Opens the Open file launcher in the routed session's side region and focuses its filter
    -- (`openPicker`): the one tab Desktop's narrow screens keep.
    local function browse(sid: string?)
        local session = if sid then Sessions:Get(sid) else Sessions:Routed()
        if session == nil then
            return
        end
        Layout:OpenPanel(PANEL, { sessionId = session.id, instance = panel.OPEN })
        env.filter:Focus()
    end

    -- Enter in the filter opens the file it found first, kept (`onSelectPermanent`); the filter
    -- keeps its text.
    env.filter.Submitted:Connect(function(text: string)
        local session = Sessions:Routed()
        local first = if session then panel.found(env, session.directory, text) else nil
        if session and first then
            panel.open(env, session.id, first)
        end
    end)

    ctx:AddPanel({
        id = PANEL,
        title = I18n:T("command.open"),
        icon = "file-tree",
        launcher = false,
        list = function(sid: string, open: { string }): { PanelTab }
            return panel.tabs(env, sid, open)
        end,
        mobile = { title = I18n:T("mobile.title"), order = 20, kind = "menu", icon = "folder" },
        render = function(view, props)
            if props.placement == "mobile" then
                return panel.narrow(env, props)
            end
            return panel.wide(env, view, props)
        end,
    })

    ctx:AddCommand({
        id = "browse",
        title = I18n:T("command.open"),
        group = I18n:T("command.category.view"),
        section = "session",
        bind = if App.Platform == "macos" then "ctrl+mod+p" else "ctrl+alt+p",
        editable = true,
        requiresSession = true,
        run = function()
            browse(nil)
        end,
    })

    -- The + menu's and New tab's Open file row (order 30).
    ctx:AddMenuItem({
        menu = "session.panel",
        id = "open",
        title = I18n:T("command.open"),
        icon = "file-tree",
        keybind = "file.browse",
        order = 30,
        when = function(sid: string): boolean
            local session = Sessions:Get(sid)
            return session ~= nil and session.projectId ~= nil
        end,
        run = function(sid: string?)
            browse(sid)
        end,
    })
end)
