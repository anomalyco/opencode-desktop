--!strict
-- Session stats: the routed session's context tokens, its spend, and its last 20 tool calls in a
-- side panel. Shows a command with a keybind and a slash form, a settings row backed by a store,
-- a host-owned text field, a cached server read, memoized rendering and a virtual list.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local format = require("@self/format")

type ToolCall = {
    id: string,
    tool: string,
    target: string,
    href: string?,
    failed: boolean,
}

local MAX_CALLS = 20

-- The server's messages are plain JSON, so they arrive as `any`.
local function recentToolCalls(messages: any, directory: string): { ToolCall }
    local calls: { ToolCall } = {}
    if type(messages) ~= "table" then
        return calls
    end
    for _, message in messages do
        -- A message's parts: `content` in the server's v2 messages, `parts` in v1.
        for _, part in message.content or message.parts or {} do
            if part.type == "tool" then
                local input = if part.state then part.state.input or {} else {}
                local file = input.filePath or input.path
                local href: string? = nil
                if type(file) == "string" and #file > 0 then
                    local absolute = file:sub(1, 1) == "/" or file:sub(2, 2) == ":"
                    href = if absolute then `file://{file}` else `file://{directory}/{file}`
                end
                table.insert(calls, {
                    id = tostring(part.id or part.callID),
                    tool = tostring(part.name or part.tool),
                    target = tostring(file or input.command or input.pattern or part.name or part.tool),
                    href = href,
                    failed = part.state ~= nil and part.state.status == "error",
                })
            end
        end
    end
    return calls
end

return plugin.defineWindow(function(ctx)
    local Sessions = ctx:GetService("Sessions")
    local Server = ctx:GetService("Server")
    local Models = ctx:GetService("Models")
    local Store = ctx:GetService("Store")
    local Layout = ctx:GetService("Layout")
    local Links = ctx:GetService("Links")
    local System = ctx:GetService("System")
    local Toast = ctx:GetService("Toast")
    local Commands = ctx:GetService("Commands")
    local Theme = ctx:GetService("Theme")

    local showSpend = Store:Value("show-spend", true)
    local filter = ctx:CreateEditor("")

    ctx:AddCommand({
        id = "open",
        title = "Session stats: Toggle panel",
        description = "Show token usage, spend and recent tool calls",
        keywords = "tokens cost tools usage stats",
        section = "session",
        bind = "mod+shift+s",
        slash = { name = "stats" },
        requiresSession = true,
        run = function()
            Layout:TogglePanel("main", { transition = "default" })
        end,
    })

    ctx:AddSetting({
        id = "show-spend",
        title = "Show session spend",
        description = "Display the session's cost in USD in the Session stats panel",
        keywords = "cost spend usd billing stats",
        render = function(view)
            return ui.switch({
                key = "session-stats-show-spend",
                label = "Show session spend",
                checked = showSpend:Get(),
                onChange = view:Handler(function(event)
                    showSpend:Set(event.checked == true)
                end),
            })
        end,
    })

    ctx:AddPanel({
        id = "main",
        title = "Session stats",
        description = "Tokens, spend and the last 20 tool calls",
        icon = "window-analytics",
        order = 35,
        render = function(view, props)
            local sid = props.sessionId
            local session = Sessions:Get(sid)
            local sessionVersion = if session then session.version else 0
            local messages = Server:Resource(`/api/session/{sid}/message?limit=60&order=desc`, sessionVersion)
            local spendVisible = showSpend:Get()
            local version =
                `{sid}:{sessionVersion}:{messages.generation}:{spendVisible}:{filter:Version()}:{Theme:Current().id}`

            return view:Memo("panel", version, function()
                local needle = filter:Text():match("^%s*(.-)%s*$") or ""
                needle = needle:lower()

                local totalTokens = 0
                local limit: number? = nil
                if session and session.usage then
                    local u = session.usage
                    totalTokens = u.input + u.output + u.reasoning + u.cacheRead + u.cacheWrite
                    if session.usageProviderId and session.usageModelId then
                        limit = Models:Get(session.usageProviderId, session.usageModelId).contextLimit
                    end
                end
                local tokenLabel = if limit and limit > 0
                    then `{format.int(totalTokens)} / {format.int(limit)} ({math.round(totalTokens / limit * 100)}%)`
                    else format.int(totalTokens)

                local metrics: { ui.Node } = {
                    ui.col({ grow = 1, gap = 3 }, {
                        ui.text("Context tokens", { size = 12, color = "text-weak" }),
                        ui.metric({
                            value = totalTokens,
                            text = tokenLabel,
                            size = 14,
                            weight = "semibold",
                            color = "text-strong",
                        }),
                    }),
                }
                if spendVisible then
                    local cost = if session then session.cost else 0
                    table.insert(
                        metrics,
                        ui.col({ grow = 1, gap = 3 }, {
                            ui.text("Session spend", { size = 12, color = "text-weak" }),
                            ui.metric({
                                value = cost,
                                text = format.usd(cost),
                                size = 14,
                                weight = "semibold",
                                color = "text-strong",
                            }),
                        })
                    )
                end

                local directory = if session and #session.directory > 0 then session.directory else props.directory
                local data = if messages.json then messages.json.data else nil
                local calls: { ToolCall } = {}
                for _, call in recentToolCalls(data, directory) do
                    local matches = needle == ""
                        or call.tool:lower():find(needle, 1, true) ~= nil
                        or call.target:lower():find(needle, 1, true) ~= nil
                    if matches then
                        table.insert(calls, call)
                        if #calls >= MAX_CALLS then
                            break
                        end
                    end
                end

                local list: ui.Node
                if #calls == 0 then
                    local message = if sid == ""
                        then "Open a session to see its tool calls."
                        elseif needle ~= "" then "No tool calls match the filter."
                        else "This session has no tool calls yet."
                    list = ui.text(message, { size = 13, color = "text-weak", py = 12 })
                else
                    list = ui.virtualList({
                        key = "session-stats-list",
                        w = "full",
                        grow = 1,
                        count = #calls,
                        estimatedRowHeight = 28,
                        gap = 2,
                        version = version,
                        rowKey = function(index)
                            return calls[index].id
                        end,
                        renderRow = function(index)
                            local call = calls[index]
                            return ui.row({
                                key = `tool-{call.id}`,
                                role = "button",
                                label = `{call.tool} {call.target}`,
                                w = "full",
                                h = 28,
                                px = 8,
                                gap = 8,
                                align = "center",
                                radius = 4,
                                hover = "surface-hover",
                                press = "surface-press",
                                hoverGroup = true,
                                onPress = view:Handler(function()
                                    if call.href then
                                        Links:Open({ href = call.href, sessionId = sid })
                                    else
                                        System:CopyText(call.target)
                                        Toast:Info("Copied tool input")
                                    end
                                end),
                            }, {
                                ui.box({
                                    w = 6,
                                    h = 6,
                                    radius = 3,
                                    fill = if call.failed then "status-danger" else "status-success",
                                }),
                                ui.text(call.tool, { size = 12, weight = "medium", mono = true, noWrap = true }),
                                ui.text(
                                    call.target,
                                    { size = 12, mono = true, color = "text-weak", truncate = true, shrink = true }
                                ),
                                ui.icon(
                                    if call.href then "arrow-up-right" else "copy",
                                    { size = 14, color = "icon-weak", revealOnHover = true }
                                ),
                            })
                        end,
                    })
                end

                return ui.col({ w = "full", h = "full", fill = "bg-base", pad = 16, gap = 14 }, {
                    ui.col({ w = "full", pad = 12, radius = 6, fill = "surface-1", border = "border-weak" }, {
                        ui.row({ w = "full", gap = 16 }, metrics),
                    }),
                    ui.row({ w = "full", gap = 8, align = "center" }, {
                        ui.field({
                            key = "session-stats-filter",
                            editor = filter,
                            placeholder = "Filter tool calls",
                            grow = 1,
                        }),
                        ui.button({
                            key = "session-stats-compact",
                            text = "Compact",
                            variant = "neutral",
                            size = "small",
                            disabled = sid == "",
                            onPress = view:Handler(function()
                                Commands:Run("session.compact")
                                Toast:Info("Asked the session to compact")
                            end),
                        }),
                    }),
                    ui.text("Recent tool calls", { size = 12, weight = "medium", color = "text-weak" }),
                    list,
                })
            end)
        end,
    })
end)
