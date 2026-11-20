--!strict
-- Desktop's built-in browser plugin (`packages/gui-plugins/src/browser`): one side-region tab per
-- page of the session (`browser:<page id>`), titled by its page with activity dot; the pane with
-- its reorganized toolbar, left-elided punycode address bar, two-view Site info popover, '...'
-- options popover + More tools submenu, Find in page bar, Refusal notices with 500 ms activation
-- delay, Device toolbar, plain-language error page, new tab page, and MCP tools.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local address = require("@self/address")
local createDevice = require("@self/device")
local failure = require("@self/failure")
local find = require("@self/find")
local history = require("@self/history")
local hostDisplay = require("@self/host_display")
local mcp = require("@self/mcp")
local options = require("@self/options")
local panel = require("@self/panel")
local refusal = require("@self/refusal")
local site = require("@self/site")

-- The panel's id: its tabs key as `browser:<page id>`, as Desktop's.
local PANEL = "main"
-- How long an agent-revealed tab holds clicks on its bars/notices (ms).
local REVEAL_GUARD_MS = 500

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Sessions = ctx:GetService("Sessions")
    local Browser = ctx:GetService("Browser")
    local Layout = ctx:GetService("Layout")
    local App = ctx:GetService("App")
    local Store = ctx:GetService("Store")
    local Contracts = ctx:GetService("Contracts")
    local System = ctx:GetService("System")
    local Toast = ctx:GetService("Toast")
    local Composer = ctx:GetService("Composer")
    local Server = ctx:GetService("Server")
    local Servers = ctx:GetService("Servers")
    local Popover = ctx:GetService("Popover")
    local Workspace = ctx:GetService("Workspace")

    -- The window's address field and find field.
    local editor = ctx:CreateEditor("")
    local findEditor = ctx:CreateEditor("")
    local stored = Store:Value("history", { visits = {} })
    local recorder = Contracts:Use("browser.history")

    local state = address.initial()
    local epochs: { [string]: address.Epoch } = {}
    -- Each page's last recorded visit, so repeated reports don't rewrite history.
    local recorded: { [string]: string } = {}
    -- Per-tab Find state, refusal notice state, transient failure notices, activity dot, reveal guard.
    local findStates: { [string]: find.State } = {}
    local activeFindTab: string? = nil
    local refusalNotices: { [string]: refusal.Notice } = {}
    local dismissedRefusals: { [string]: number } = {}
    local failureNotices: { [string]: failure.Notice } = {}
    local nextNoticeId = 0
    local unseenTabs: { [string]: boolean } = {}
    local revealUntil: { [string]: number } = {}
    local panelHandle: Contribution? = nil

    local function refresh()
        if panelHandle then
            panelHandle:Refresh()
        end
    end

    local function visits(): { history.Visit }
        local value = stored:Get()
        return if type(value) == "table" and type(value.visits) == "table" then value.visits else {}
    end

    local device = if App.Channel == "local" or App.Channel == "dev" then createDevice(ctx, I18n, Popover, refresh) else nil

    local function pageUrl(tab: BrowserTab): string
        local url = tab:Url()
        return if url == "about:blank" then "" else url
    end

    local function isServerFile(url: string): boolean
        return string.sub(url, 1, 12) == "workspace://" or string.sub(url, 1, 11) == "artifact://" or string.sub(url, 1, 13) == "oc-preview://"
    end

    local function workspaceSourcePath(url: string): string?
        if string.sub(url, 1, 8) == "file:///" then
            local path = string.sub(url, 8)
            -- Strip query/fragment and decode simple `%20`.
            path = string.match(path, "^([^?#]*)") or path
            path = string.gsub(path, "%%20", " ")
            if App.Platform == "windows" and string.match(path, "^/[A-Za-z]:") ~= nil then
                path = string.sub(path, 2)
            end
            return path
        end
        return nil
    end

    -- Engine capability probes (gracefully false on older engines without them).
    local function engineHas(flag: string): boolean
        local eng = Browser:Engine() :: any
        return eng ~= nil and eng[flag] == true
    end

    -- Page `tab`'s report counts, folded with what it reports now.
    local function epoch(tab: BrowserTab): address.Epoch
        local e = address.fold(epochs[tab.Id], pageUrl(tab), tab:Loading(), tab:Rejected())
        epochs[tab.Id] = e
        return e
    end

    local function showFailure(tabId: string, text: string, lasting: boolean, retry: (() -> ())?)
        nextNoticeId += 1
        local id = nextNoticeId
        failureNotices[tabId] = { id = id, text = text, lasting = lasting, retry = retry }
        if not lasting then
            task.delay(failure.NOTICE_MS / 1000, function()
                local cur = failureNotices[tabId]
                if cur and cur.id == id then
                    failureNotices[tabId] = nil
                    refresh()
                end
            end)
        end
        refresh()
    end

    local function run(effects: { address.Effect })
        for _, effect in effects do
            if effect.type == "select" then
                editor:SelectAll()
            elseif effect.type == "blur" then
                editor:Blur()
            elseif effect.type == "navigate" then
                local tab = Browser:Get(effect.tab)
                if tab then
                    tab:Navigate(effect.url)
                end
            end
        end
    end

    local function dispatch(event: address.Event)
        local nextState, effects = address.step(state, event)
        state = nextState
        run(effects)
        refresh()
    end

    -- Puts page `tab`'s address in the field and the keyboard there, the whole address selected.
    local function edit(tab: BrowserTab)
        local nextState, effects = address.step(state, { type = "focused", tab = tab.Id, epoch = epoch(tab) })
        state = nextState
        local e = state.edit
        if e.status == "editing" then
            editor:SetText(e.text)
        end
        editor:Focus()
        run(effects)
        refresh()
    end

    local function editing(): BrowserTab?
        local e = state.edit
        return if e.status == "editing" then Browser:Get(e.tab) else nil
    end

    editor.FocusChanged:Connect(function(focused: boolean)
        if not focused then
            dispatch({ type = "blurred" })
        end
    end)
    editor.Changed:Connect(function(value: string)
        local e = state.edit
        if e.status ~= "editing" or e.text == value then
            return
        end
        dispatch({ type = "input", value = value, deleting = #value < #e.text, visits = visits() })
    end)
    editor.Submitted:Connect(function()
        local tab = editing()
        if tab then
            dispatch({ type = "submitted", tab = tab.Id, epoch = epoch(tab), visits = visits() })
        end
    end)
    editor.KeyPressed:Connect(function(key: string)
        dispatch({ type = "key", key = key, visits = visits() })
    end)

    -- Find in page state management (`find.luau`, ready for WS-1 Phase 1c engine `FindHandler`).
    local function getFind(tabId: string): find.State
        return findStates[tabId] or find.initial()
    end

    local function runFind(tab: BrowserTab, query: string, forward: boolean, findNext: boolean)
        local tAny = tab :: any
        if type(tAny.Find) == "function" then
            tAny:Find(query, forward, findNext)
        end
    end

    local function stopFind(tab: BrowserTab)
        local tAny = tab :: any
        if type(tAny.StopFinding) == "function" then
            tAny:StopFinding(true)
        end
    end

    local function openFind(tab: BrowserTab)
        local next = find.step(getFind(tab.Id), { type = "open" })
        findStates[tab.Id] = next
        activeFindTab = tab.Id
        if findEditor:Text() ~= next.query then
            findEditor:SetText(next.query)
        end
        findEditor:Focus()
        findEditor:SelectAll()
        if next.query ~= "" then
            runFind(tab, next.query, true, false)
        end
        refresh()
    end

    local function closeFind(tab: BrowserTab)
        findStates[tab.Id] = find.step(getFind(tab.Id), { type = "close" })
        if activeFindTab == tab.Id then
            activeFindTab = nil
        end
        stopFind(tab)
        findEditor:Blur()
        refresh()
    end

    findEditor.Changed:Connect(function(value: string)
        local tabId = activeFindTab
        if tabId == nil then
            return
        end
        local cur = getFind(tabId)
        if cur.status == "closed" or cur.query == value then
            return
        end
        findStates[tabId] = find.step(cur, { type = "query", query = value })
        local tab = Browser:Get(tabId)
        if tab then
            if value == "" then
                stopFind(tab)
            else
                runFind(tab, value, true, false)
            end
        end
        refresh()
    end)
    findEditor.Submitted:Connect(function()
        local tabId = activeFindTab
        if tabId == nil then
            return
        end
        local tab = Browser:Get(tabId)
        local cur = getFind(tabId)
        if tab and cur.query ~= "" then
            runFind(tab, cur.query, true, true)
        end
    end)
    findEditor.KeyPressed:Connect(function(key: string)
        if key == "Escape" and activeFindTab ~= nil then
            local tab = Browser:Get(activeFindTab)
            if tab then
                closeFind(tab)
            end
        end
    end)

    -- Syncs a tab's policy refusal into a timestamped `refusal.Notice` for the 500 ms guard.
    local function syncRefusal(tab: BrowserTab)
        local code = tab:Refusal()
        local rej = tab:Rejected()
        if code == nil then
            refusalNotices[tab.Id] = nil
            return
        end
        if dismissedRefusals[tab.Id] == rej then
            return
        end
        local token = `{tab.Id}:{rej}:{code}`
        local existing = refusalNotices[tab.Id]
        if existing and existing.token == token then
            return
        end
        refusalNotices[tab.Id] = {
            token = token,
            origin = "",
            text = I18n:T(failure.refusalKey(code)),
            target = nil,
            action = nil,
            shownAtMs = System:Now(),
        }
    end

    -- A page that moved: its counts, refusal notice, activity dot, history once loaded, and pane.
    Browser.Changed:Connect(function(id: string)
        local tab = Browser:Get(id)
        if tab == nil then
            epochs[id] = nil
            recorded[id] = nil
            findStates[id] = nil
            refusalNotices[id] = nil
            dismissedRefusals[id] = nil
            failureNotices[id] = nil
            unseenTabs[id] = nil
            revealUntil[id] = nil
            if activeFindTab == id then
                activeFindTab = nil
            end
            refresh()
            return
        end
        local prev = epochs[id]
        local prevUrl = if prev then prev.url else ""
        epoch(tab)
        syncRefusal(tab)
        local url = pageUrl(tab)
        -- Mark unseen activity dot when an existing background tab navigates while another tab is selected.
        local selected = Browser:Selected(nil)
        if prev ~= nil and url ~= "" and url ~= prevUrl and (selected == nil or selected.Id ~= id) then
            unseenTabs[id] = true
        end
        if not tab:Loading() and history.recordable(url) and not isServerFile(url) then
            local visit = `{url}\n{tab:Title()}`
            local keeper = recorder:Get()
            if recorded[id] ~= visit and keeper then
                recorded[id] = visit
                keeper.remember(url, tab:Title())
            end
        end
        refresh()
    end)

    -- A picked element goes into the composer as a pill naming its `@eN` ref and selector.
    Browser.Picked:Connect(function(pick: BrowserPick)
        Composer:InsertPill(`browser:{pick.ref}`, `{pick.ref} ({pick.selector})`)
    end)

    -- A loopback address opens on the machine that runs the server; the system browser here
    -- reaches it only when the server runs here too.
    local function reachable(url: string): boolean
        local scheme, host = address.parts(url)
        local lower = string.lower(scheme)
        if lower == "file://" then
            return false
        end
        if lower ~= "http://" and lower ~= "https://" then
            return false
        end
        local name = string.match(host, "^%[[^%]]*%]") or string.match(host, "^[^:]*") or host
        return Server:IsLocal() or not address.loopback(name)
    end

    -- Where the keyboard goes when a new blank tab opens.
    type Keyboard = "address" | "kept"
    local kept: { [string]: boolean } = {}
    local opening: Keyboard? = nil

    local function openNew(sid: string?, keyboard: Keyboard)
        local session = if sid then Sessions:Get(sid) else Sessions:Routed()
        if session == nil or not Browser:Available() then
            return
        end
        opening = keyboard
        local tab = Browser:OpenAsync({ url = "about:blank", sessionId = session.id, background = keyboard == "kept" })
        opening = nil
        if tab and keyboard == "address" then
            edit(tab)
        elseif tab then
            kept[tab.Id] = true
        end
    end

    -- Site information: the page it describes, and its two-view cookie/cache state machine.
    local siteState: site.State = site.initial
    local sitePage: { tab: string, url: string }? = nil

    local function siteDispatch(event: site.Event)
        local nextState, effects = site.step(siteState, event)
        siteState = nextState
        for _, effect in effects do
            local described = sitePage
            if effect.type == "notify-clear-failed" then
                Toast:Info(I18n:T("site.clearFailed"))
            elseif described then
                local attempt = effect.attempt
                local tab = Browser:Get(described.tab)
                if effect.type == "load" then
                    task.delay(site.LOAD_TIMEOUT, function()
                        siteDispatch({ type = "timeout", phase = "loading", attempt = attempt })
                    end)
                    task.spawn(function()
                        local ok, count = pcall(function()
                            return if tab then tab:SiteCookiesAsync() else nil
                        end)
                        siteDispatch(if ok and type(count) == "number" then { type = "load.succeeded", attempt = attempt, cookies = count } else { type = "load.failed", attempt = attempt })
                    end)
                elseif effect.type == "clear-cache" then
                    task.delay(site.CLEAR_TIMEOUT, function()
                        siteDispatch({ type = "timeout", phase = "clearing", attempt = attempt })
                    end)
                    task.spawn(function()
                        local ok = pcall(function()
                            local tAny = tab :: any
                            if tAny == nil or type(tAny.ClearCacheAsync) ~= "function" then
                                error("unsupported", 0)
                            end
                            tAny:ClearCacheAsync()
                        end)
                        siteDispatch(if ok then { type = "clear.succeeded", attempt = attempt } else { type = "clear.failed", attempt = attempt })
                    end)
                else
                    task.delay(site.CLEAR_TIMEOUT, function()
                        siteDispatch({ type = "timeout", phase = "clearing", attempt = attempt })
                    end)
                    task.spawn(function()
                        local ok = pcall(function()
                            if tab == nil then
                                error("gone", 0)
                            end
                            tab:ClearSiteAsync()
                        end)
                        siteDispatch(if ok then { type = "clear.succeeded", attempt = attempt } else { type = "clear.failed", attempt = attempt })
                    end)
                end
            end
        end
        refresh()
    end

    local siteEnv: site.Env = {
        I18n = I18n,
        state = function()
            return siteState
        end,
        dispatch = siteDispatch,
        server = function()
            if Server:IsLocal() then
                return nil
            end
            for _, s in Servers:List() do
                if s.current then
                    return s.name
                end
            end
            return nil
        end,
        canClearCache = function()
            return engineHas("clearCache")
        end,
    }

    ctx:AddPopover({
        id = "site",
        title = I18n:T("site.info"),
        render = function(view, props)
            local data: { [string]: any } = props.data or {}
            return site.render(siteEnv, view, if type(data.url) == "string" then data.url else "")
        end,
    })

    local function act(what: string, tab: BrowserTab?, arg: any?)
        if what == "settings" then
            Popover:Close()
            Layout:OpenSettings("browser")
            return
        end
        if tab == nil then
            return
        end
        if what == "device" and device then
            device.toggle(tab)
        elseif what == "edit" then
            edit(tab)
        elseif what == "go" then
            dispatch({ type = "navigated", tab = tab.Id, url = arg :: string, epoch = epoch(tab) })
        elseif what == "zoom" then
            Popover:Close()
            tab:SetZoom(arg :: any)
        elseif what == "page" then
            Popover:Close()
            local url = pageUrl(tab)
            local shownHost = hostDisplay.displayHost(url)
            local title = tab:Title()
            local labelText = if title ~= "" then title elseif shownHost ~= "" then shownHost else url
            Composer:InsertPill(`browser:{url}`, `{labelText} ({url})`)
        elseif what == "link.copy" then
            Popover:Close()
            local url = pageUrl(tab)
            if url ~= "" and not isServerFile(url) then
                System:CopyText(url)
                Toast:Info(I18n:T("common.copied"))
            end
        elseif what == "external" then
            Popover:Close()
            local url = pageUrl(tab)
            if reachable(url) then
                System:OpenExternal(url)
            else
                Toast:Error(I18n:T("menu.failed"))
            end
        elseif what == "reveal" then
            Popover:Close()
            local path = workspaceSourcePath(pageUrl(tab))
            if path then
                System:Reveal(path)
            end
        elseif what == "view.source" then
            Popover:Close()
            local path = workspaceSourcePath(pageUrl(tab))
            local session = Sessions:Routed()
            if path and session then
                local rel = Workspace:Relative(path, session.directory)
                if rel then
                    Workspace:OpenFile(session.id, rel)
                end
            end
        elseif what == "inspect" then
            Popover:Close()
            tab:Inspect()
        elseif what == "hard.reload" then
            Popover:Close()
            local tAny = tab :: any
            if type(tAny.ReloadIgnoreCache) == "function" then
                tAny:ReloadIgnoreCache()
            else
                tab:Reload()
            end
        elseif what == "history.clear" then
            Popover:Close()
            local keeper = recorder:Get()
            if keeper then
                keeper.clear()
            end
        elseif what == "find.open" then
            Popover:Close()
            openFind(tab)
        elseif what == "find.next" then
            local cur = getFind(tab.Id)
            if cur.query ~= "" then
                runFind(tab, cur.query, true, true)
            end
        elseif what == "find.previous" then
            local cur = getFind(tab.Id)
            if cur.query ~= "" then
                runFind(tab, cur.query, false, true)
            end
        elseif what == "find.close" then
            closeFind(tab)
        elseif what == "refusal.action" then
            local notice = arg :: refusal.Notice?
            if notice and notice.target then
                if notice.action == "open" then
                    System:OpenExternal(notice.target)
                else
                    System:CopyText(notice.target)
                    Toast:Info(I18n:T("common.copied"))
                end
            end
            dismissedRefusals[tab.Id] = tab:Rejected()
            refusalNotices[tab.Id] = nil
            refresh()
        elseif what == "refusal.dismiss" then
            dismissedRefusals[tab.Id] = tab:Rejected()
            refusalNotices[tab.Id] = nil
            refresh()
        elseif what == "failure.dismiss" then
            failureNotices[tab.Id] = nil
            refresh()
        elseif what == "options" then
            local url = pageUrl(tab)
            local srcPath = workspaceSourcePath(url)
            local session = Sessions:Routed()
            local canSource = srcPath ~= nil and session ~= nil and Workspace:Relative(srcPath, session.directory) ~= nil
            local menuData: options.MenuData = {
                tabId = tab.Id,
                url = url,
                zoom = tab:Zoom(),
                inspectable = url ~= "" and tab:LoadError() == nil,
                suspended = false,
                failed = tab:LoadError() ~= nil,
                serverFile = isServerFile(url),
                canOpenExternal = reachable(url),
                canRevealFile = srcPath ~= nil and Server:IsLocal(),
                canViewSource = canSource,
                canFind = true,
                canHardReload = true,
                canClearHistory = #visits() > 0,
            }
            Popover:Toggle({
                id = "options",
                anchorKey = "browser-options",
                side = "bottom",
                align = "end",
                gap = 4,
                data = menuData :: any,
            })
        elseif what == "site" then
            local url = pageUrl(tab)
            if sitePage == nil or sitePage.tab ~= tab.Id or sitePage.url ~= url then
                siteState = site.initial
            end
            sitePage = { tab = tab.Id, url = url }
            if not Popover:IsOpen("site") then
                siteDispatch({ type = "opened" })
            else
                siteDispatch({ type = "closed" })
            end
            Popover:Toggle({ id = "site", anchorKey = "browser-site-anchor", side = "bottom", align = "start", gap = 8, data = { url = url, tab = tab.Id } })
        end
    end

    local optionsCb: options.Callbacks = {
        I18n = I18n,
        act = function(action: string, tabId: string, arg: any?)
            act(action, Browser:Get(tabId), arg)
        end,
    }

    ctx:AddPopover({
        id = "options",
        title = I18n:T("options"),
        render = function(view, props)
            return options.renderMain(optionsCb, view, (props.data or {}) :: any)
        end,
    })

    ctx:AddPopover({
        id = "more-tools",
        title = I18n:T("menu.more"),
        render = function(view, props)
            return options.renderMore(optionsCb, view, (props.data or {}) :: any)
        end,
    })

    local env: panel.Env = {
        device = if device then device.render else nil,
        deviceOpen = if device then device.isOpen else nil,
        I18n = I18n,
        Browser = Browser,
        Server = Server,
        System = System,
        menuTrigger = true,
        editor = editor,
        findEditor = findEditor,
        state = function()
            return state
        end,
        epoch = epoch,
        visits = visits,
        reachable = reachable,
        siteOpen = function()
            return Popover:IsOpen("site")
        end,
        optionsOpen = function()
            return Popover:IsOpen("options")
        end,
        refusalNotice = function(tab: BrowserTab)
            return refusalNotices[tab.Id]
        end,
        failureNotice = function(tab: BrowserTab)
            return failureNotices[tab.Id]
        end,
        findView = function(tab: BrowserTab)
            return find.view(getFind(tab.Id))
        end,
        revealGuarded = function(tab: BrowserTab)
            local untilMs = revealUntil[tab.Id]
            return untilMs ~= nil and System:Now() < untilMs
        end,
        act = act,
    }

    -- A tab's label: "New tab" for a blank page, else the page's title, else its canonical host/URL.
    local function label(info: BrowserTabInfo): string
        if info.url == "" then
            return I18n:T("tab.new")
        end
        if info.title ~= "" and info.title ~= "New tab" then
            return info.title
        end
        local host = hostDisplay.displayHost(info.url)
        return if host ~= "" then host else info.url
    end

    local function tabText(tab: BrowserTab): string
        local url = pageUrl(tab)
        if url == "" then
            return I18n:T("tab.new")
        end
        local title = tab:Title()
        if title ~= "" and title ~= "New tab" then
            return title
        end
        local host = hostDisplay.displayHost(url)
        return if host ~= "" then host else url
    end

    panelHandle = ctx:AddPanel({
        id = PANEL,
        title = I18n:T("tab.title"),
        icon = "globe",
        launcher = false,
        transient = true,
        list = function(sid: string, open: { string }): { PanelTab }
            local out: { PanelTab } = {}
            for _, info in Browser:List(sid, open) do
                if info.open then
                    table.insert(out, { instance = info.id, title = label(info), icon = "globe", fallback = true })
                end
            end
            return out
        end,
        label = function(_view, props)
            local id = props.instance or ""
            local tab = if id ~= "" then Browser:Get(id) else nil
            if tab == nil then
                return nil
            end
            local title = tabText(tab)
            local children: { ui.Node } = {
                ui.icon("globe", { size = 16, color = "icon-weak" }),
                ui.text(title, {
                    size = 13,
                    weight = "regular",
                    lineHeight = 16,
                    color = if props.active then "text-chrome" else "text-muted",
                    truncate = true,
                    shrink = true,
                    minW = 0,
                    maxW = 160,
                }),
            }
            if unseenTabs[id] then
                table.insert(children, ui.box({
                    key = `browser-tab-dot-{id}`,
                    role = "img",
                    label = I18n:T("tab.activity"),
                    w = 6,
                    h = 6,
                    radius = 3,
                    fill = "status-info",
                }))
            end
            return ui.row({ gap = 6, align = "center", minW = 0, shrink = true }, children)
        end,
        render = function(view, props)
            return panel.render(env, view, props)
        end,
        -- A blank tab starts in its address field, like a browser's new tab; focusing clears unseen dot.
        onFocus = function(props)
            local tab = if props.instance then Browser:Get(props.instance) else nil
            if tab == nil then
                return
            end
            if unseenTabs[tab.Id] then
                unseenTabs[tab.Id] = nil
                refresh()
            end
            if opening == "kept" or kept[tab.Id] then
                kept[tab.Id] = nil
                return
            end
            if props.restored ~= true and pageUrl(tab) == "" then
                edit(tab)
            end
        end,
        onClose = function(props)
            local tab = if props.instance then Browser:Get(props.instance) else nil
            if tab then
                tab:Close()
            end
        end,
        closedTab = function(props): ClosedTab?
            local tab = if props.instance then Browser:Get(props.instance) else nil
            local url = if tab then pageUrl(tab) else ""
            if url == "" then
                return nil
            end
            local host = hostDisplay.displayHost(url)
            if host == "" then
                local _, raw = address.parts(url)
                host = raw
            end
            return { icon = "globe", detail = host, data = url }
        end,
        restore = function(props, data: string?)
            if data and data ~= "" then
                Browser:OpenAsync({ url = data, sessionId = props.sessionId })
            end
        end,
    })

    ctx:AddCommand({
        id = "open",
        title = I18n:T("command.open"),
        group = I18n:T("command.category.view"),
        bind = if App.Platform == "macos" then "ctrl+mod+b" else "ctrl+alt+b",
        editable = true,
        requiresSession = true,
        run = function()
            if not Browser:Available() then
                return false
            end
            openNew(nil, if Layout:RegionFocused() then "address" else "kept")
            return nil
        end,
    })

    -- The routed session's page on screen, for F5.
    ctx:AddCommand({
        id = "reload",
        title = I18n:T("command.reload"),
        group = I18n:T("command.category.view"),
        bind = "f5",
        editable = true,
        requiresSession = true,
        run = function()
            local tab = Browser:Selected(nil)
            if tab == nil or pageUrl(tab) == "" then
                return false
            end
            tab:Reload()
            return nil
        end,
    })

    local function scoped(id: string, bind: string, hidden: boolean, title: string, handler: (tab: BrowserTab) -> boolean)
        ctx:AddCommand({
            id = id,
            title = title,
            group = I18n:T("command.category.view"),
            bind = bind,
            hidden = hidden,
            editable = true,
            requiresSession = true,
            run = function()
                local tab = Browser:Focused()
                if tab == nil or not handler(tab) then
                    return false
                end
                return nil
            end,
        })
    end
    scoped("reloadPage", "mod+r", true, I18n:T("command.reload"), function(tab)
        tab:Reload()
        return true
    end)
    scoped("hardReload", if App.Platform == "macos" then "mod+alt+r" else "ctrl+f5", false, I18n:T("menu.hardReload"), function(tab)
        if pageUrl(tab) == "" then
            return false
        end
        act("hard.reload", tab)
        return true
    end)
    scoped("find", "mod+f", false, I18n:T("find.open"), function(tab)
        if pageUrl(tab) == "" or tab:LoadError() ~= nil then
            return false
        end
        openFind(tab)
        return true
    end)
    scoped("find.next", "mod+g", false, I18n:T("find.next"), function(tab)
        if pageUrl(tab) == "" or tab:LoadError() ~= nil then
            return false
        end
        act("find.next", tab)
        return true
    end)
    scoped("find.previous", "mod+shift+g", false, I18n:T("find.previous"), function(tab)
        if pageUrl(tab) == "" or tab:LoadError() ~= nil then
            return false
        end
        act("find.previous", tab)
        return true
    end)
    if device then
        scoped("toggleToolbar", "mod+shift+m", false, I18n:T("command.toggleToolbar"), function(tab)
            if pageUrl(tab) == "" or tab:LoadError() ~= nil then
                return false
            end
            device.toggle(tab)
            return true
        end)
    end
    scoped("address", "mod+l", false, I18n:T("command.address"), function(tab)
        edit(tab)
        return true
    end)
    scoped("close", "mod+w", true, I18n:T("command.close"), function(tab)
        tab:Close()
        return true
    end)
    scoped("newTab", "mod+t", true, I18n:T("command.newTab"), function()
        openNew(nil, "address")
        return true
    end)
    scoped("inspect", "mod+shift+c", false, I18n:T("command.inspect"), function(tab)
        if pageUrl(tab) == "" then
            return false
        end
        tab:Inspect()
        return true
    end)

    -- The + menu's and New tab's Browser row, between Review and Terminal (order 15).
    ctx:AddMenuItem({
        menu = "session.panel",
        id = "open",
        title = I18n:T("tab.title"),
        icon = "globe",
        keybind = "browser.open",
        order = 15,
        when = function(sid: string): boolean
            return Sessions:Get(sid) ~= nil and Browser:Available()
        end,
        hover = function()
            Browser:Prepare()
        end,
        run = function(sid: string?)
            openNew(sid, "address")
        end,
    })

    -- Expose an internal contract so agent tools or link routing can trigger a reveal click guard
    -- or show an external app-scheme refusal notice inside the browser pane.
    Contracts:Provide("browser.chrome", {
        markAgentActivity = function(tabId: string, revealed: boolean?)
            local selected = Browser:Selected(nil)
            if selected == nil or selected.Id ~= tabId then
                unseenTabs[tabId] = true
            end
            if revealed then
                revealUntil[tabId] = System:Now() + REVEAL_GUARD_MS
                task.delay(REVEAL_GUARD_MS / 1000 + 0.02, function()
                    refresh()
                end)
            end
            refresh()
        end,
        showFailure = function(tabId: string, text: string, lasting: boolean?)
            showFailure(tabId, text, lasting == true, nil)
        end,
        showExternalNotice = function(tabId: string, target: string, source: refusal.Source?)
            local action = refusal.externalTarget(target, source)
            if action == nil then
                return false
            end
            local origin = if source == "conversation" then I18n:T("notice.conversation") else I18n:T("notice.page")
            nextNoticeId += 1
            refusalNotices[tabId] = {
                token = `ext:{tabId}:{nextNoticeId}`,
                origin = origin,
                text = I18n:T("refused.scheme"),
                target = target,
                action = action,
                shownAtMs = System:Now(),
            }
            refresh()
            return true
        end,
    })

    mcp.add(ctx, Browser, Layout)
end)
