--!strict
-- Desktop's built-in Session details plugin (`packages/gui-plugins/src/details`): the `[grid]`
-- button in the session header (its `session.header` slot, order 20) and the popover it opens
-- (mod+shift+y), with the server's Extensions: its MCP servers (each with its status, its switch
-- or Sign in, and its error), its plugins and its skills, each list beside its row; the MCP
-- error and sign-in dialogs; and in a narrow window the same card as More › Session details,
-- each row opening its list as a view of the drawer. Offered for sessions of a project only.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local lists = require("@self/lists")
local mcp = require("@self/mcp")

-- What the app's session header keeps at its end for its popovers to hang from (Desktop's
-- `Popover.Anchor`, 12 from the end, 10 down, 28 tall).
local ANCHOR = "session-header-anchor"

-- The card's heading text; frozen, so the host decodes it once.
local HEADING = table.freeze({
    size = 13,
    weight = "regular" :: FontWeight,
    lineHeight = 16,
    color = "text-faint",
    truncate = true,
    shrink = true,
    dir = "auto",
} :: UiTextProps)

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Sessions = ctx:GetService("Sessions")
    local Servers = ctx:GetService("Servers")
    local Server = ctx:GetService("Server")
    local Popover = ctx:GetService("Popover")
    local Commands = ctx:GetService("Commands")
    local Theme = ctx:GetService("Theme")
    local Layout = ctx:GetService("Layout")
    local Dialog = ctx:GetService("Dialog")
    local System = ctx:GetService("System")
    local Toast = ctx:GetService("Toast")
    lists.init(Theme)
    local changed = ctx:CreateSignal()
    local editor = ctx:CreateEditor()
    local state: lists.State = { opens = 0, parsed = {}, actions = {} }
    local function redraw()
        changed:Fire()
    end
    local env: lists.Env
    local mcpEnv: mcp.Env = {
        Server = Server,
        System = System,
        Dialog = Dialog,
        I18n = I18n,
        Toast = Toast,
        editor = editor,
        states = {},
        signIn = { current = nil },
        location = function(directory)
            return lists.location(env, directory)
        end,
        servers = function(json)
            return lists.servers(json) :: any
        end,
        redraw = redraw,
    }
    env = {
        Server = Server,
        Servers = Servers,
        I18n = I18n,
        Popover = Popover,
        Layout = Layout,
        Links = ctx:GetService("Links"),
        Commands = Commands,
        System = System,
        Toast = Toast,
        state = state,
        mcp = mcpEnv,
        redraw = redraw,
    }

    editor.Changed:Connect(function(text)
        mcp.signInStep(mcpEnv, { type = "paste.changed", value = text })
    end)
    editor.Submitted:Connect(function()
        mcp.signInStep(mcpEnv, { type = "paste.submitted" })
    end)

    -- Desktop's `DetailsHeader`: only a session of a project has the details.
    local function ofProject(session: Session?): boolean
        return session ~= nil and session.projectId ~= nil
    end

    -- Each open reads the lists afresh, and a copied path's mark goes with the popover.
    local function toggle()
        if not Popover:IsOpen("main") then
            state.opens += 1
            for key, action in state.actions do
                if action.status == "copied" then
                    state.actions[key] = { status = "idle", issued = action.attempt }
                end
            end
        end
        Popover:Toggle({ id = "main", anchorKey = ANCHOR, side = "bottom", align = "end", gap = 8 })
    end

    -- The card's heading: Extensions, or the server's name when the app lists more than one.
    local function heading(): string
        local all = Servers:List()
        if #all < 2 then
            return I18n:T("server")
        end
        local current = Servers:Current()
        for _, s in all do
            if s.key == current then
                return s.name
            end
        end
        return I18n:T("server")
    end

    -- A narrow window's rows are 44 tall, the heading's too (`max-width: 767px`).
    local function headingRow(mobile: boolean?): ui.Node
        return ui.row({ w = "full", minH = if mobile then 44 else 28, pl = 12, pr = 36, py = 4, align = "center" }, {
            ui.text(heading(), HEADING),
        })
    end

    -- Present while the routed session belongs to a project, as Desktop's popover owns it.
    ctx:AddCommand({
        id = "toggle",
        title = I18n:T("command.toggle"),
        group = I18n:T("category.view"),
        section = "session",
        bind = "mod+shift+y",
        keywords = "view details extensions mcp plugins skills",
        requiresSession = true,
        enabled = function()
            return ofProject(Sessions:Routed())
        end,
        run = function()
            toggle()
            return nil
        end,
    })

    -- The header's button, for a session of a project.
    ctx:AddSlot({
        id = "details",
        at = "session.header",
        order = 20,
        render = function(view, props)
            local session = Sessions:Get(props.sessionId or "")
            if session == nil or not ofProject(session) then
                return nil
            end
            local open = Popover:IsOpen("main")
            local keys = Commands:Keybind("details.toggle")
            local tip = if keys then `{I18n:T("tooltip")}  {keys}` else I18n:T("tooltip")
            local version = `{open}:{tip}:{Theme:Current().id}:{Theme:IsDark()}`
            return view:Memo(`trigger-{session.id}`, version, function()
                -- The memo keys its root, so the button's own key sits a level down.
                return ui.row({}, {
                    ui.row({
                        key = "details-trigger",
                        role = "button",
                        label = I18n:T("title"),
                        help = tip,
                        w = 28,
                        h = 28,
                        center = true,
                        radius = 6,
                        hover = "surface-hover",
                        press = "surface-press",
                        hoverTransition = "instant",
                        fill = if open then "surface-press" else nil,
                        onPress = view:Handler(toggle),
                    }, {
                        ui.icon("window-analytics", { size = 16, color = if open then "icon-base" else "icon-weak" }),
                    }),
                })
            end)
        end,
    })

    -- The popover: the Extensions card. Each row shows its list beside the card (`list`).
    ctx:AddPopover({
        id = "main",
        title = I18n:T("title"),
        render = function(_view, _props)
            local session = Sessions:Routed()
            if session == nil then
                return nil
            end
            local rows: { ui.Node } = {}
            for _, service in lists.all() do
                local load = lists.read(env, service, session.directory)
                local failed, count = lists.summary(load)
                table.insert(
                    rows,
                    ui.flyoutRow({
                        key = `details-{service}`,
                        text = I18n:T(service),
                        icon = lists.icon(service),
                        detail = if count > 0 then tostring(count) else nil,
                        alert = failed,
                        popover = "list",
                        props = { service = service },
                        side = "left",
                    })
                )
            end
            return ui.col({ key = "session-summary-popover" }, {
                ui.col({
                    key = "session-summary-card",
                    role = "group",
                    label = I18n:T("title"),
                    w = 280,
                    pad = 2,
                    gap = 2,
                    radius = 6,
                    fill = "surface-1",
                    shadow = "floating",
                }, {
                    headingRow(),
                    ui.col({ key = "session-summary-rows", w = "full", gap = 2 }, rows),
                }),
            })
        end,
    })

    -- A row's list, beside the card (`session-service-flyout`).
    ctx:AddPopover({
        id = "list",
        render = function(view, props)
            local session = Sessions:Routed()
            local data = props.data
            if session == nil or data == nil then
                return nil
            end
            local service = data.service :: lists.Service
            return ui.col({
                key = "details-flyout",
                w = 280,
                py = 2,
                radius = 6,
                fill = "surface-1",
                shadow = "floating",
            }, { lists.pane(env, view, service, session.directory) })
        end,
    })

    -- `McpErrorDialog`: a failed server's error, following its status in the server's list.
    ctx:AddDialog({
        id = "mcp-error",
        title = I18n:T("mcp.error.status"),
        render = function(view, props)
            local data = props.data
            if type(data) ~= "table" then
                return nil
            end
            local load = lists.read(env, "mcp", data.directory)
            local status = nil
            if load.kind == "ready" then
                for _, item in load.items do
                    if item.name == data.name then
                        status = { status = item.status, error = item.error }
                    end
                end
            end
            return mcp.renderError(mcpEnv, view, data, status)
        end,
        onDismiss = function()
            local open = mcpEnv.states
            for directory, s in open do
                if s.dialog.status == "open" then
                    mcp.dispatch(mcpEnv, directory, { type = "error.dismissed", attempt = s.dialog.attempt })
                end
            end
        end,
    })

    -- `McpSignInDialog`: waits for the browser's sign-in, or takes its redirect URL.
    ctx:AddDialog({
        id = "mcp-sign-in",
        render = function(view, _props)
            return mcp.renderSignIn(mcpEnv, view)
        end,
        onDismiss = function()
            mcp.signInDismissed(mcpEnv)
        end,
    })

    -- The narrow window's details drawer: the card full width, each row opening its list as a view
    -- of the drawer (`useDrawer().open`), for sessions of a project (subagents included).
    ctx:AddPanel({
        id = "main",
        title = I18n:T("title"),
        icon = "info",
        launcher = false,
        mobile = { title = I18n:T("title"), order = 50, kind = "drawer", icon = "info" },
        enabled = function(sessionId)
            return ofProject(Sessions:Get(sessionId))
        end,
        list = function()
            return {}
        end,
        render = function(view, props)
            local session = Sessions:Get(props.sessionId)
            if session == nil or not ofProject(session) then
                return nil
            end
            local dark = Theme:IsDark()
            local nested = props.drawer
            if nested and type(nested.service) == "string" then
                -- `max-width: 639px`: an MCP row stands 44 tall (the window is 2 wider than the pane).
                return lists.drawer(env, view, nested.service, session.directory, props.width + 2 < 640, dark)
            end
            local rows: { ui.Node } = {}
            for _, service in lists.all() do
                table.insert(
                    rows,
                    ui.row({
                        key = `details-drawer-{service}`,
                        role = "button",
                        label = I18n:T(service),
                        w = "full",
                        minH = 44,
                        pl = 12,
                        pr = 8,
                        py = 4,
                        gap = 8,
                        align = "center",
                        radius = 4,
                        hover = "surface-hover",
                        hoverTransition = "instant",
                        onPress = view:Handler(function()
                            state.opens += 1
                            Layout:OpenDrawerView(I18n:T(service), { service = service })
                        end),
                    }, {
                        ui.icon(lists.icon(service), { size = 16, color = "icon-weak" }),
                        ui.text(I18n:T(service), {
                            size = 13,
                            weight = "regular",
                            lineHeight = 16,
                            color = "text-chrome",
                            truncate = true,
                            grow = 1,
                            shrink = true,
                            minW = 0,
                        }),
                        ui.icon("chevron-right", { size = 16, color = "icon-weak", ml = 4 }),
                    })
                )
            end
            return ui.col({ key = "session-summary-panel", w = "full", gap = 8 }, {
                ui.col({
                    key = "session-summary-card",
                    w = "full",
                    pad = 2,
                    gap = 2,
                    radius = 6,
                    fill = if dark then "surface-1" else "bg-base",
                    shadow = "raised",
                }, {
                    headingRow(true),
                    ui.col({ key = "session-summary-rows", w = "full", gap = 2 }, rows),
                }),
            })
        end,
    })
end)
