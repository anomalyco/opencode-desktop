--!strict
-- Workbench: the native nodes driven from Luau. The host keeps every heavy engine (the terminal
-- emulator, the diff engine, the code view, the file tree, the browser, Markdown and UI outlines)
-- and Luau owns tabs, state, commands, MCP tools and contracts.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")

return plugin.defineWindow(function(ctx)
    local Terminal = ctx:GetService("Terminal")
    local Workspace = ctx:GetService("Workspace")
    local Diff = ctx:GetService("Diff")
    local Browser = ctx:GetService("Browser")
    local Sessions = ctx:GetService("Sessions")
    local Store = ctx:GetService("Store")
    local Layout = ctx:GetService("Layout")
    local Popover = ctx:GetService("Popover")
    local Menu = ctx:GetService("Menu")
    local Contracts = ctx:GetService("Contracts")
    local Console = ctx:GetService("Console")

    -- A union-typed store value: cast through `any`, or Luau's older solver widens "split" to string.
    local diffMode = Store:Value("diff-mode", "split" :: any) :: StoreValue<DiffMode>
    local openTerminals = Store:SessionValue("terminals", {} :: { string })
    local terminals: { [string]: PtySession } = {}
    local diffs: { [string]: DiffDocument } = {}
    local trees: { [string]: FileTreeModel } = {}
    local preview: BrowserTab? = nil
    local changed = ctx:CreateSignal()

    local function terminalFor(sessionId: string, instance: string): PtySession
        local existing = terminals[instance] or Terminal:Attach(instance)
        if existing then
            terminals[instance] = existing
            return existing
        end
        local session = Sessions:Get(sessionId)
        local pty = Terminal:Create({ directory = if session then session.directory else "", title = "Scratch" })
        pty.Exited:Connect(function(code)
            Console:Log(`terminal {pty.Id} exited with {code or "no code"}`)
            changed:Fire()
        end)
        terminals[pty.Id] = pty
        return pty
    end

    local function openDiff(sessionId: string): DiffDocument
        local diff = Diff:Open({ sessionId = sessionId, source = "git" })
        diff.Changed:Connect(function()
            changed:Fire()
        end)
        diffs[sessionId] = diff
        return diff
    end

    local function openTerminal(sessionId: string): PtySession
        local pty = terminalFor(sessionId, "")
        local ids = openTerminals:Get(sessionId)
        table.insert(ids, pty.Id)
        openTerminals:Set(sessionId, ids)
        Layout:OpenPanel("shell", { sessionId = sessionId, instance = pty.Id })
        return pty
    end

    ctx:AddPanel({
        id = "shell",
        title = "Scratch terminal",
        icon = "console",
        list = function(_sessionId, open)
            local tabs: { PanelTab } = {}
            for i, id in open do
                table.insert(tabs, { instance = id, title = `Terminal {i}` })
            end
            return tabs
        end,
        render = function(_view, props)
            local pty = terminalFor(props.sessionId, props.instance or "")
            return ui.terminalSurface({
                key = `pty-{pty.Id}`,
                session = pty,
                w = "full",
                h = "full",
                focused = props.active,
            })
        end,
        label = function(_view, props)
            local pty = terminals[props.instance or ""]
            return ui.row({ gap = 6, align = "center" }, {
                ui.icon("console", { size = 14, color = if props.active then "icon-strong" else "icon-weak" }),
                ui.text(if pty then pty:Title() else "Terminal", { size = 12, truncate = true }),
            })
        end,
        onClose = function(props)
            local pty = terminals[props.instance or ""]
            if pty then
                pty:Close()
                terminals[pty.Id] = nil
            end
        end,
    })

    ctx:AddPanel({
        id = "changes",
        title = "Changes",
        icon = "review",
        wide = true,
        render = function(view, props)
            local sid = props.sessionId
            local diff = diffs[sid] or openDiff(sid)
            local session = Sessions:Get(sid)
            local tree = trees[sid]
            if not tree and session then
                tree = Workspace:OpenTree(session.directory)
                trees[sid] = tree
            end
            local mode: DiffMode = diffMode:Get()
            return view:Memo("changes", `{sid}:{diff:Version()}:{mode}:{diffMode:Version()}`, function()
                local totals = diff:Totals()
                local header = ui.row(
                    { key = "changes-header", w = "full", h = 36, px = 12, gap = 8, align = "center" },
                    {
                        ui.metric({ value = totals.added, text = `+{totals.added}`, color = "status-success" }),
                        ui.metric({ value = totals.removed, text = `-{totals.removed}`, color = "status-danger" }),
                        ui.spacer(),
                        ui.iconButton({
                            key = "changes-options",
                            icon = "settings",
                            label = "Diff options",
                            onPress = view:Handler(function()
                                Popover:Toggle({
                                    id = "options",
                                    anchorKey = "changes-options",
                                    side = "bottom",
                                    align = "end",
                                    gap = 4,
                                })
                            end),
                        }),
                        ui.iconButton({
                            key = "changes-source",
                            icon = "chevron-down",
                            label = "Compare",
                            onPress = view:Handler(function()
                                local picked = Menu:OpenAsync({
                                    anchorKey = "changes-source",
                                    align = "end",
                                    items = {
                                        { kind = "group", title = "Compare" },
                                        { id = "git", title = "Working tree" },
                                        { id = "branch", title = "Branch" },
                                        { kind = "separator" },
                                        { id = "turn", title = "Last turn" },
                                    },
                                })
                                if picked == "git" or picked == "branch" or picked == "turn" then
                                    diff:SetSource(picked)
                                end
                            end),
                        }),
                    }
                )
                local body: { ui.Node } = {}
                if tree then
                    table.insert(
                        body,
                        ui.fileTree({
                            key = "changes-tree",
                            tree = tree,
                            w = 240,
                            h = "full",
                            onSelect = view:Handler(function(event)
                                if event.path then
                                    Layout:OpenPanel("changes", { sessionId = sid, transition = "none" })
                                end
                            end),
                        })
                    )
                end
                table.insert(
                    body,
                    ui.diffView({ key = "changes-diff", diff = diff, mode = mode, grow = 1, h = "full" })
                )
                return ui.col({ w = "full", h = "full" }, {
                    header,
                    ui.divider(),
                    ui.row({ w = "full", grow = 1, minH = 0 }, body),
                })
            end)
        end,
    })

    ctx:AddPopover({
        id = "options",
        title = "Diff options",
        width = 220,
        render = function(view)
            local mode = diffMode:Get()
            return ui.col({ pad = 8, gap = 4 }, {
                ui.radio({
                    text = "Split",
                    description = "Old and new side by side",
                    checked = mode == "split",
                    onChange = view:Handler(function()
                        diffMode:Set("split")
                    end),
                }),
                ui.radio({
                    text = "Unified",
                    checked = mode == "unified",
                    onChange = view:Handler(function()
                        diffMode:Set("unified")
                    end),
                }),
                ui.divider(),
                ui.flyoutRow({ text = "About diffs", icon = "help", popover = "about" }),
            })
        end,
    })

    ctx:AddPopover({
        id = "about",
        render = function()
            return ui.markdown({
                source = "**Split** shows old and new side by side.\n\n**Unified** shows one column.",
                pad = 12,
            })
        end,
    })

    ctx:AddFileViewer({
        id = "luau",
        title = "Luau source",
        extensions = { "lua", "luau" },
        render = function(_view, props)
            return ui.codeView({
                key = props.path,
                doc = Workspace:OpenDocument(props.path),
                w = "full",
                h = "full",
                lineWrap = false,
            })
        end,
    })

    ctx:AddCodeBlock({
        id = "outline",
        languages = { "ui" },
        render = function(_view, props)
            return ui.genUi({ source = props.code, w = "full" })
        end,
    })

    ctx:AddCodeBlockAction({
        id = "run",
        languages = { "sh", "bash", "shell" },
        icon = "console",
        title = "Run in a scratch terminal",
        run = function(props)
            local session = Sessions:Routed()
            if session then
                openTerminal(session.id):Write(props.code .. "\n")
            end
        end,
    })

    ctx:AddLinkHandler({
        id = "localhost",
        priority = 10,
        match = function(link)
            return link.href:match("^https?://localhost[:/]") ~= nil
        end,
        open = function(link)
            if preview then
                preview:Navigate(link.href)
            else
                preview = Browser:Open({ url = link.href, sessionId = link.sessionId })
            end
            Layout:OpenPanel("preview", { sessionId = link.sessionId, background = link.background })
        end,
    })

    ctx:AddPanel({
        id = "preview",
        title = "Preview",
        icon = "globe",
        launcher = false,
        render = function()
            local tab = preview
            if not tab then
                return ui.col({ w = "full", h = "full", center = true }, {
                    ui.text("Open a localhost link to preview it.", { color = "text-weak" }),
                })
            end
            return ui.browserSurface({ key = `page-{tab.Id}`, tab = tab, w = "full", h = "full" })
        end,
    })

    ctx:AddSlot({
        id = "count",
        at = "session.panel.end",
        render = function(_view, props)
            local count = if props.sessionId then #openTerminals:Get(props.sessionId) else 0
            if count == 0 then
                return nil
            end
            return ui.pill({ text = `{count} terminals`, icon = "console" })
        end,
    })

    ctx:AddMcpTool({
        name = "open_terminal",
        description = "Opens a scratch terminal in the routed session and types a command into it.",
        inputSchema = {
            type = "object",
            properties = { command = { type = "string" } },
        },
        call = function(args)
            local session = Sessions:Routed()
            if not session then
                error("no session is open")
            end
            local pty = openTerminal(session.id)
            if type(args.command) == "string" then
                pty:Write(args.command .. "\n")
            end
            return { terminal = pty.Id }
        end,
    })

    Contracts:Provide("workbench.terminals", {
        count = function(): number
            local n = 0
            for _ in terminals do
                n += 1
            end
            return n
        end,
    })

    ctx.Disposing:Connect(function()
        for _, diff in diffs do
            diff:Close()
        end
        for _, tree in trees do
            tree:Close()
        end
    end)
end)
