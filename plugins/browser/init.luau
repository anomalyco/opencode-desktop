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
local extensions = require("@self/extensions")
local failure = require("@self/failure")
local find = require("@self/find")
local history = require("@self/history")
local hostDisplay = require("@self/host_display")
local mcp = require("@self/mcp")
local options = require("@self/options")
local panel = require("@self/panel")
local refusal = require("@self/refusal")
local savedTabs = require("@self/saved_tabs")
local settings = require("@self/settings")
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
    local Dialog = ctx:GetService("Dialog")
    local Menu = ctx:GetService("Menu")
    local Images = ctx:GetService("Images")
    local Workspace = ctx:GetService("Workspace")

    -- The window's address field, find field, and extension store link field.
    local editor = ctx:CreateEditor("")
    local findEditor = ctx:CreateEditor("")
    local extLinkEditor = ctx:CreateEditor("")
    local stored = Store:Value("history", { visits = {} })
    local prefsStore = Store:Value("preferences", { links = true, agentPages = true, advanced = false, welcomed = false })
    local sitePermsStore = Store:Value("rune.sitePermissions", {})
    local extensionsStore = Store:Value("extensions", {})
    local savedTabsStore = Store:Value("savedTabs", { sessions = {} })
    local recorder = Contracts:Use("browser.history")
    local extContract = Contracts:Use("browser.extensions")
    local profileContract = Contracts:Use("browser.profile")

    local state = address.initial()
    local epochs: { [string]: address.Epoch } = {}
    -- Each page's last recorded visit, so repeated reports don't rewrite history.
    local recorded: { [string]: string } = {}
    local recordedUrl: { [string]: string } = {}
    local typedNavs: { [string]: string } = {}
    local syncingEditor = false
    local adoptedHistory = false
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

    local function isPreviewUrl(url: string): boolean
        local lower = string.lower(url)
        return string.sub(lower, 1, 7) == "file://" or isServerFile(lower)
    end

    local function isPreviewTab(tab: BrowserTab): boolean
        local tAny = tab :: any
        if type(tAny.PreviewKind) == "function" and tAny:PreviewKind() ~= nil then
            return true
        end
        return isPreviewUrl(pageUrl(tab))
    end

    local function decodePercent(s: string): string
        return (string.gsub(s, "%%(%x%x)", function(hex)
            local byte = tonumber(hex, 16)
            return if byte and byte >= 32 then string.char(byte) else `%{hex}`
        end))
    end

    local function localFilePath(url: string): string?
        if string.sub(url, 1, 8) == "file:///" then
            local path = string.sub(url, 8)
            path = string.match(path, "^([^?#]*)") or path
            path = decodePercent(path)
            if App.Platform == "windows" and string.match(path, "^/[A-Za-z]:") ~= nil then
                path = string.sub(path, 2)
            end
            return path
        end
        return nil
    end

    -- Resolves workspace-relative source path for BOTH local (`file:///...`) and remote
    -- (`workspace://<session_id>/<rel>`) previews so "Show the file's code" works everywhere.
    local function workspaceSourceRel(url: string, session: Session?): string?
        if session == nil then
            return nil
        end
        local localPath = localFilePath(url)
        if localPath ~= nil then
            return Workspace:Relative(localPath, session.directory)
        end
        local prefix = `workspace://{string.lower(session.id)}/`
        if string.sub(string.lower(url), 1, #prefix) == prefix then
            local rest = string.sub(url, #prefix + 1)
            rest = string.match(rest, "^([^?#]*)") or rest
            if rest ~= "" then
                return decodePercent(rest)
            end
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

    local function ensureAdopted()
        if adoptedHistory then
            return
        end
        local bAny = Browser :: any
        if type(bAny.HistoryAdopt) == "function" then
            adoptedHistory = true
            bAny:HistoryAdopt(visits())
        end
    end

    local function setEditorText(text: string)
        syncingEditor = true
        editor:SetText(text)
        syncingEditor = false
    end

    local dispatch: (event: address.Event) -> ()

    local function run(effects: { address.Effect })
        local bAny = Browser :: any
        for _, effect in effects do
            if effect.type == "select" then
                editor:SelectAll()
            elseif effect.type == "blur" then
                editor:Blur()
            elseif effect.type == "prepare" then
                ensureAdopted()
                if type(bAny.HistoryPrepare) == "function" then
                    bAny:HistoryPrepare()
                end
            elseif effect.type == "set-text" then
                setEditorText(effect.text)
            elseif effect.type == "ask" then
                ensureAdopted()
                local pages: { history.HistoryPage } = {}
                if type(bAny.HistorySearch) == "function" then
                    local raw = bAny:HistorySearch(effect.query, address.HISTORY_ANSWER)
                    if type(raw) == "table" then
                        pages = raw :: { history.HistoryPage }
                    end
                end
                dispatch({ type = "answered", attempt = effect.attempt, pages = pages, visits = visits() })
            elseif effect.type == "forget" then
                if type(bAny.HistoryForget) == "function" then
                    bAny:HistoryForget(effect.url)
                end
                local keeper = recorder:Get()
                if keeper and type(keeper.forget) == "function" then
                    keeper.forget(effect.url)
                end
            elseif effect.type == "navigate" then
                local tab = Browser:Get(effect.tab)
                if tab then
                    if effect.typed then
                        typedNavs[effect.tab] = effect.url
                    end
                    tab:Navigate(effect.url)
                end
            end
        end
    end

    dispatch = function(event: address.Event)
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
            setEditorText(e.text)
        end
        editor:Focus()
        run(effects)
        refresh()
    end

    local function editing(): BrowserTab?
        local e = state.edit
        return if e.status == "editing" then Browser:Get(e.tab) else nil
    end

    local editorFocused = false
    editor.FocusChanged:Connect(function(focused: boolean)
        editorFocused = focused
        if not focused then
            task.delay(0.12, function()
                if not editorFocused then
                    dispatch({ type = "blurred" })
                end
            end)
        end
    end)
    editor.Changed:Connect(function(value: string)
        if syncingEditor then
            return
        end
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
        local tab = editing()
        if tab then
            dispatch({ type = "key", key = key, epoch = epoch(tab), visits = visits() })
        end
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
    local function syncRefusal(tab: BrowserTab, navigated: boolean)
        local code = tab:Refusal()
        local rej = tab:Rejected()
        local existing = refusalNotices[tab.Id]
        if code == nil or code == "browser.controlled.inherited_permission" then
            if existing ~= nil and (navigated or string.sub(existing.token, 1, 4) ~= "app:") then
                refusalNotices[tab.Id] = nil
            end
            return
        end
        if dismissedRefusals[tab.Id] == rej then
            return
        end
        local token = `{tab.Id}:{rej}:{code}`
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
            recordedUrl[id] = nil
            typedNavs[id] = nil
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
        local url = pageUrl(tab)
        local navigated = prev ~= nil and url ~= prevUrl
        syncRefusal(tab, navigated)
        if navigated and findStates[id] ~= nil and findStates[id].status ~= "closed" then
            findStates[id] = find.step(findStates[id], { type = "close" })
            if activeFindTab == id then
                activeFindTab = nil
            end
        end
        -- Mark unseen activity dot when an existing background tab navigates while another tab is selected.
        local selected = Browser:Selected(nil)
        if prev ~= nil and url ~= "" and url ~= prevUrl and (selected == nil or selected.Id ~= id) then
            unseenTabs[id] = true
        end
        local tAny = tab :: any
        local isPrivate = type(tAny.IsPrivate) == "function" and tAny:IsPrivate() == true
        local previewTab = isPreviewTab(tab)
        -- Private/isolated preview tabs and internal server files NEVER write to history.
        if not isPrivate and not previewTab and not tab:Loading() and history.recordable(url) and not isServerFile(url) then
            local title = tab:Title()
            local liveFav: string? = if type(tAny.Favicon) == "function" then tAny:Favicon() else nil
            local visit = `{url}\n{title}`
            local visitWithIcon = `{visit}\n{liveFav or ""}`
            local keeper = recorder:Get()
            local bAny = Browser :: any
            if recorded[id] ~= visit then
                local sameUrl = recordedUrl[id] == url
                recorded[id] = visit
                recordedUrl[id] = url
                if keeper then
                    keeper.remember(url, title, liveFav)
                end
                ensureAdopted()
                if sameUrl then
                    if type(bAny.HistoryRetitle) == "function" then
                        bAny:HistoryRetitle(url, title)
                    end
                else
                    local wasTyped = typedNavs[id] ~= nil
                    typedNavs[id] = nil
                    if type(bAny.HistoryVisit) == "function" then
                        bAny:HistoryVisit(url, title, wasTyped, id)
                    end
                end
            elseif liveFav ~= nil and liveFav ~= "" and recorded[`icon:{id}`] ~= visitWithIcon then
                recorded[`icon:{id}`] = visitWithIcon
                if keeper then
                    keeper.remember(url, title, liveFav)
                end
            end
        end
        -- Sync saved tabs for immediate restore & loading hold (`UP-100` `#65067`).
        -- Isolated tabs (`Preview`, `Artifact`, `Private`, `RemoteServer`) NEVER persist to disk.
        local routed = Sessions:Routed()
        if routed ~= nil then
            local curSaved = savedTabs.normalize(savedTabsStore:Get())
            local reported: { savedTabs.Reported } = {}
            for _, info in Browser:List(routed.id, {}) do
                if info.open and history.recordable(info.url) then
                    local live = Browser:Get(info.id)
                    local liveAny = live :: any
                    local skip = live ~= nil
                        and (isPreviewTab(live) or (type(liveAny.IsPrivate) == "function" and liveAny:IsPrivate() == true))
                    if not skip then
                        table.insert(reported, {
                            id = info.id,
                            url = info.url,
                            title = if live then live:Title() else info.title,
                            loading = if live then live:Loading() else false,
                        })
                    end
                end
            end
            local nextSaved = savedTabs.save(curSaved, routed.id, reported)
            if nextSaved ~= nil then
                curSaved = nextSaved
                savedTabsStore:Set(curSaved :: any)
            end
            if not isPrivate and not previewTab then
                local fav = if type(tAny.Favicon) == "function" then tAny:Favicon() else nil
                if type(fav) == "string" and fav ~= "" then
                    local withIcon = savedTabs.saveIcon(curSaved, routed.id, id, fav)
                    if withIcon ~= nil then
                        savedTabsStore:Set(withIcon :: any)
                    end
                end
            end
        end
        refresh()
    end)

    -- A picked element goes into the composer as a pill naming its `@eN` ref and selector.
    Browser.Picked:Connect(function(pick: BrowserPick)
        Composer:InsertPill(`browser:{pick.ref}`, `{pick.ref} ({pick.selector})`)
    end)

    -- A loopback address opens on the machine that runs the server; the system browser here
    -- reaches it only when the server runs here too (authoritative Rust policy via CanOpenExternally).
    local function reachable(url: string, tab: BrowserTab?): boolean
        local tAny = tab :: any
        if tAny ~= nil and type(tAny.CanOpenExternally) == "function" then
            return tAny:CanOpenExternally() == true
        end
        local scheme, host = address.parts(url)
        local lower = string.lower(scheme)
        if lower ~= "http://" and lower ~= "https://" then
            return false
        end
        if string.find(host, "@", 1, true) ~= nil then
            return false
        end
        local name = string.match(host, "^%[[^%]]*%]") or string.match(host, "^[^:]*") or host
        name = string.gsub(name, "%.+$", "")
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
                    if not site.isWeb(described.url) then
                        siteDispatch({ type = "load.succeeded", attempt = attempt, cookies = 0 })
                        continue
                    end
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
        elseif what == "forget" then
            dispatch({ type = "forget", url = arg :: string?, visits = visits() })
            editor:Focus()
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
            if reachable(url, tab) then
                System:OpenExternal(url)
            else
                Toast:Error(I18n:T("menu.failed"))
            end
        elseif what == "reveal" then
            Popover:Close()
            local path = localFilePath(pageUrl(tab))
            if path then
                System:Reveal(path)
            end
        elseif what == "view.source" then
            Popover:Close()
            local session = Sessions:Routed()
            local rel = workspaceSourceRel(pageUrl(tab), session)
            if rel and session then
                local wsAny = Workspace :: any
                if type(wsAny.OpenFileSource) == "function" then
                    wsAny:OpenFileSource(session.id, rel)
                else
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
            local bAny = Browser :: any
            if type(bAny.HistoryClear) == "function" then
                bAny:HistoryClear()
            end
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
                -- Plugin refusal notices are strictly copy-only; OS app launches occur exclusively
                -- through the native host-rendered review bar (`surfaces/browser/prompts/app_link.rs`).
                System:CopyText(notice.target)
                Toast:Info(I18n:T("common.copied"))
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
            local localPath = localFilePath(url)
            local session = Sessions:Routed()
            local canSource = workspaceSourceRel(url, session) ~= nil
            local eng = Browser:Engine() :: any
            local canFind = eng == nil or eng.state ~= "ready" or eng.find == true
            local menuData: options.MenuData = {
                tabId = tab.Id,
                url = url,
                zoom = tab:Zoom(),
                inspectable = url ~= "" and tab:LoadError() == nil,
                suspended = tab:Refusal() == "browser.proxy.required",
                failed = tab:LoadError() ~= nil,
                serverFile = isServerFile(url),
                canOpenExternal = reachable(url, tab),
                canRevealFile = localPath ~= nil and Server:IsLocal(),
                canViewSource = canSource,
                canFind = canFind,
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
            local v = find.view(getFind(tab.Id))
            if v == nil then
                return nil
            end
            local tAny = tab :: any
            if v.query ~= "" and type(tAny.FindResult) == "function" then
                local res = tAny:FindResult()
                if type(res) == "table" and type(res.active) == "number" and type(res.total) == "number" then
                    return { query = v.query, result = { active = res.active, total = res.total } }
                end
            end
            return v
        end,
        revealGuarded = function(tab: BrowserTab)
            local untilMs = revealUntil[tab.Id]
            return untilMs ~= nil and System:Now() < untilMs
        end,
        act = act,
    }

    -- A tab's label: "New tab" for a blank or unnamed link-opened page, else live/saved title, else host/URL.
    local function label(sid: string, info: BrowserTabInfo): string
        if info.url == "" then
            return I18n:T("tab.new")
        end
        local saved = savedTabs.find(savedTabs.normalize(savedTabsStore:Get()), sid, info.id)
        local live = Browser:Get(info.id)
        if live ~= nil then
            local liveAny = live :: any
            if type(liveAny.Unnamed) == "function" and liveAny:Unnamed() == true then
                return I18n:T("tab.new")
            end
        end
        local held = savedTabs.tabTitle(info.url, info.title, if live then live:Loading() else true, saved)
        if held ~= nil then
            return held
        end
        local host = hostDisplay.displayHost(info.url)
        return if host ~= "" then host else info.url
    end

    local function tabText(sid: string, tab: BrowserTab): string
        local url = pageUrl(tab)
        local tAny = tab :: any
        if url == "" or (type(tAny.Unnamed) == "function" and tAny:Unnamed() == true) then
            return I18n:T("tab.new")
        end
        local saved = savedTabs.find(savedTabs.normalize(savedTabsStore:Get()), sid, tab.Id)
        local held = savedTabs.tabTitle(url, tab:Title(), tab:Loading(), saved)
        if held ~= nil then
            return held
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
                    table.insert(out, { instance = info.id, title = label(sid, info), icon = "globe", fallback = true })
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
            local sid = props.sessionId or ""
            local title = tabText(sid, tab)
            local labelColor: Color = if props.active then "text-chrome" else "text-muted"
            local iconNode: ui.Node
            local tAny = tab :: any
            local liveFav = if type(tAny.Favicon) == "function" then tAny:Favicon() else nil
            local saved = savedTabs.find(savedTabs.normalize(savedTabsStore:Get()), sid, id)
            local fav = savedTabs.tabIcon(pageUrl(tab), liveFav, saved)
            if fav ~= nil then
                iconNode = ui.image({ src = fav, w = 16, h = 16 }, {
                    ui.icon("globe", { size = 16, color = labelColor }),
                })
            else
                iconNode = ui.icon("globe", { size = 16, color = labelColor })
            end
            local children: { ui.Node } = {
                iconNode,
                ui.text(title, {
                    size = 13,
                    weight = "regular",
                    lineHeight = 16,
                    tracking = -0.04,
                    tabular = true,
                    noWrap = true,
                    color = labelColor,
                    truncate = true,
                    shrink = true,
                    minW = 0,
                    maxW = 160,
                }),
            }
            local tAny = tab :: any
            local asking = type(tAny.Asking) == "function" and tAny:Asking() == true
            local unseen = not asking
                and (unseenTabs[id] == true or (type(tAny.Unseen) == "function" and tAny:Unseen() == true))
            -- Official `renderer.tsx:536-538`: amber `prompt.tab` ("Waiting for you") takes precedence
            -- over blue `tab.activity` ("The agent used this tab"), both 6×6 rounded dots.
            if asking then
                table.insert(children, ui.box({
                    key = `browser-tab-dot-{id}`,
                    role = "img",
                    label = I18n:T("prompt.tab"),
                    w = 6,
                    h = 6,
                    radius = 3,
                    fill = "status-warning",
                }))
            elseif unseen then
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
            if tab == nil or isPreviewTab(tab) then
                return nil
            end
            local tAny = tab :: any
            if type(tAny.IsPrivate) == "function" and tAny:IsPrivate() == true then
                return nil
            end
            local url = pageUrl(tab)
            if url == "" or not history.recordable(url) then
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

    local function scoped(id: string, bind: string, hidden: boolean, title: string, handler: (tab: BrowserTab) -> boolean, follows: string?)
        ctx:AddCommand({
            id = id,
            title = title,
            group = I18n:T("command.category.view"),
            bind = bind,
            follows = follows,
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
    -- Ctrl+W is the shell's `tab.close`: Settings > Shortcuts lists that one (Desktop's `follows`).
    scoped("close", "mod+w", true, I18n:T("command.close"), function(tab)
        tab:Close()
        return true
    end, "tab.close")
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
            -- External contract notices are copy-only: OS app launches require a single-use
            -- host-minted review token from `Browser:TakeAppLinkReview()` (never a plugin call).
            refusalNotices[tabId] = {
                token = `ext:{tabId}:{nextNoticeId}`,
                origin = origin,
                text = I18n:T("refused.scheme"),
                target = target,
                action = "copy",
                shownAtMs = System:Now(),
            }
            refresh()
            return true
        end,
    })

    local function preferences(): settings.Preferences
        return settings.normalizePreferences(prefsStore:Get())
    end

    local function updatePreferences(patch: { [string]: any })
        local nextPrefs: { [string]: any } = table.clone(preferences())
        for k, v in patch do
            nextPrefs[k] = v
        end
        prefsStore:Set(nextPrefs)
    end

    local function sitePermissionsList(): { settings.SiteDecision }
        local raw = sitePermsStore:Get()
        if type(raw) ~= "table" then
            return {}
        end
        local out: { settings.SiteDecision } = {}
        for _, item in raw :: { any } do
            if type(item) == "table" and type(item.origin) == "string" and string.match(string.lower(item.origin), "^https?://") ~= nil and type(item.permission) == "string" and (item.setting == "allow" or item.setting == "block") then
                table.insert(out, {
                    origin = item.origin,
                    permission = item.permission,
                    setting = item.setting,
                })
            end
        end
        return out
    end

    local function setSitePermission(origin: string, permission: string, setting: "allow" | "block")
        if string.match(string.lower(origin), "^https?://") == nil then
            return
        end
        local list = sitePermissionsList()
        local updated = false
        for _, d in list do
            if d.origin == origin and d.permission == permission then
                d.setting = setting
                updated = true
                break
            end
        end
        if not updated then
            table.insert(list, { origin = origin, permission = permission, setting = setting })
        end
        sitePermsStore:Set(list :: any)
    end

    local function removeSitePermission(origin: string, permission: string)
        local list = sitePermissionsList()
        local nextList: { settings.SiteDecision } = {}
        for _, d in list do
            if not (d.origin == origin and d.permission == permission) then
                table.insert(nextList, d)
            end
        end
        sitePermsStore:Set(nextList :: any)
    end

    local function storedExtensions(): { any }
        local raw = extensionsStore:Get()
        return if type(raw) == "table" then raw :: { any } else {}
    end

    local function extNativeAvailable(): boolean
        local bAny = Browser :: any
        return type(bAny.ExtAvailable) == "function" and bAny:ExtAvailable() == true
    end

    -- Sync verified on-disk startup extension specs once on window boot (never discovers other browsers).
    if extNativeAvailable() then
        local bAny = Browser :: any
        if type(bAny.ExtSync) == "function" then
            bAny:ExtSync(storedExtensions(), false)
        end
    end

    local function settingsCapabilities(): settings.Capabilities
        local prof = profileContract:Get()
        local ext = extContract:Get()
        local avail = Browser:Available()
        return {
            available = avail,
            import = avail,
            extensions = extNativeAvailable() or (type(ext) == "table" and type(ext.renderSettings) == "function"),
            remoteSignIns = avail and type((Browser :: any).ShareSignIns) == "function",
            cookieFile = type(prof) == "table" and type(prof.pickCookieFile) == "function",
        }
    end

    local settingsHandle: Contribution? = nil
    local importDialogHandle: Contribution? = nil
    local extDialogHandle: Contribution? = nil
    local clearNoticeText: string? = nil
    -- Browsers are listed only while Settings > Browser draws (never at startup), read again once a
    -- listing is a few seconds old, so a visit sees browsers installed since the last one.
    local importSources: { settings.ImportBrowser } = {}
    local importSourcesAt: number? = nil
    -- The profile the import dialog is for, what its last import said, and each row's last outcome.
    local importTarget: { browser: string, profile: string, label: string }? = nil
    local importFailure: settings.ImportMessage? = nil
    local importResults: { [string]: settings.ImportMessage } = {}
    local importIcons: { [string]: ImageId | false } = {}
    local extDialogState: extensions.DialogState = {
        busy = nil,
        installingId = nil,
        choices = {},
        selectedKey = nil,
        candidates = {},
        candidatesLoaded = false,
        devOpen = false,
        review = nil,
        showAllSites = false,
        approving = false,
        message = nil,
    }

    local function listImportSources(): { settings.ImportBrowser }
        local now = System:Now()
        if importSourcesAt == nil or now - importSourcesAt > 5000 then
            local bAny = Browser :: any
            importSources = if type(bAny.HistoryImportSources) == "function" then (bAny:HistoryImportSources() :: any) or {} else {}
            importSourcesAt = now
        end
        return importSources
    end

    local function refreshSettings()
        if settingsHandle then
            settingsHandle:Refresh()
        end
    end

    local function refreshExtDialog()
        if extDialogHandle then
            extDialogHandle:Refresh()
        end
    end

    extensionsStore.Changed:Connect(function()
        refreshSettings()
    end)

    extLinkEditor.Changed:Connect(function()
        refreshExtDialog()
    end)

    local function reloadExtCandidates()
        local choice = extensions.findChoice(extDialogState.choices, extDialogState.selectedKey)
        if choice == nil then
            extDialogState.candidates = {}
            extDialogState.candidatesLoaded = true
            return
        end
        local bAny = Browser :: any
        local list = if type(bAny.ExtBrowserList) == "function"
            then bAny:ExtBrowserList(choice.sourceId, choice.profileId, storedExtensions())
            else {}
        extDialogState.candidates = if type(list) == "table" then list :: any else {}
        extDialogState.candidatesLoaded = true
    end

    local function openAddExtensionDialog()
        local bAny = Browser :: any
        local rawSources = if type(bAny.ExtBrowserSources) == "function" then bAny:ExtBrowserSources() else {}
        local sources: { extensions.BrowserSource } = if type(rawSources) == "table" then rawSources :: any else {}
        local choices = extensions.flattenChoices(sources)
        extDialogState = {
            busy = nil,
            installingId = nil,
            choices = choices,
            selectedKey = if #choices > 0 then extensions.choiceKey(choices[1]) else nil,
            candidates = {},
            candidatesLoaded = false,
            devOpen = false,
            review = nil,
            showAllSites = false,
            approving = false,
            message = nil,
        }
        extLinkEditor:SetText("")
        reloadExtCandidates()
        Dialog:Open("add-extension")
        extLinkEditor:Focus()
    end

    local function handleStageResult(res: any)
        extDialogState.busy = nil
        extDialogState.installingId = nil
        if type(res) == "table" and res.type == "review" and type(res.review) == "table" then
            extDialogState.review = res.review :: any
            extDialogState.showAllSites = false
            extDialogState.message = nil
        elseif type(res) == "table" and res.type == "cancelled" then
            -- User dismissed folder dialog or superseded an older stage.
        elseif type(res) == "table" and res.type == "failed" then
            extDialogState.message = {
                text = extensions.failureMessage(I18n, res.reason),
                error = true,
                compatibility = nil,
            }
        else
            extDialogState.message = {
                text = I18n:T("settings.failed.install"),
                error = true,
                compatibility = nil,
            }
        end
        refreshExtDialog()
    end

    local function startStoreInstall()
        if extDialogState.busy ~= nil or extDialogState.approving then
            return
        end
        local url = string.match(extLinkEditor:Text(), "^%s*(.-)%s*$") or ""
        if url == "" then
            return
        end
        extDialogState.busy = "install-store"
        extDialogState.installingId = nil
        extDialogState.message = nil
        extLinkEditor:SetText("")
        refreshExtDialog()
        task.spawn(function()
            local bAny = Browser :: any
            local ok, res = pcall(function()
                return bAny:ExtStageStoreAsync(url, storedExtensions())
            end)
            handleStageResult(if ok then res else nil)
        end)
    end

    local function startBrowserInstall(sourceId: string, profileId: string, extId: string)
        if extDialogState.busy ~= nil or extDialogState.approving then
            return
        end
        extDialogState.busy = "install-browser"
        extDialogState.installingId = extId
        extDialogState.message = nil
        refreshExtDialog()
        task.spawn(function()
            local bAny = Browser :: any
            local ok, res = pcall(function()
                return bAny:ExtStageBrowserAsync(sourceId, profileId, extId, storedExtensions())
            end)
            handleStageResult(if ok then res else nil)
        end)
    end

    local function startFolderInstall()
        if extDialogState.busy ~= nil or extDialogState.approving then
            return
        end
        extDialogState.busy = "install-folder"
        extDialogState.installingId = nil
        extDialogState.message = nil
        refreshExtDialog()
        local pickTitle = I18n:T("settings.extensions.folder.pick")
        task.spawn(function()
            local bAny = Browser :: any
            local ok, res = pcall(function()
                return bAny:ExtStageFolderAsync(pickTitle, storedExtensions())
            end)
            handleStageResult(if ok then res else nil)
        end)
    end

    local function approveExtReview(token: string)
        if extDialogState.approving then
            return
        end
        extDialogState.approving = true
        refreshExtDialog()
        task.spawn(function()
            local bAny = Browser :: any
            local ok, res = pcall(function()
                return bAny:ExtApproveAsync(token, storedExtensions())
            end)
            extDialogState.approving = false
            extDialogState.review = nil
            extDialogState.showAllSites = false
            if ok and type(res) == "table" and res.type == "installed" and type(res.records) == "table" then
                extensionsStore:Set(res.records)
                if type(bAny.ExtSync) == "function" then
                    bAny:ExtSync(res.records, true)
                end
                if type(res.extension) == "table" then
                    extDialogState.message = extensions.installedMessage(I18n, res.extension :: any)
                end
                reloadExtCandidates()
                refreshSettings()
            else
                local reason = if ok and type(res) == "table" then res.reason else nil
                extDialogState.message = {
                    text = extensions.failureMessage(I18n, reason),
                    error = true,
                    compatibility = nil,
                }
            end
            refreshExtDialog()
        end)
    end

    local function cancelExtReview()
        local bAny = Browser :: any
        if type(bAny.ExtCancelReview) == "function" then
            bAny:ExtCancelReview()
        end
        extDialogState.review = nil
        extDialogState.showAllSites = false
        refreshExtDialog()
    end

    ctx:AddPopover({
        id = "ext-browser-profile",
        title = I18n:T("settings.extensions.browser.select"),
        render = function(view)
            return extensions.renderProfileMenu(extDialogState.choices, extDialogState.selectedKey, view, function(key: string)
                Popover:Close()
                extDialogState.selectedKey = key
                reloadExtCandidates()
                refreshExtDialog()
            end)
        end,
    })

    extDialogHandle = ctx:AddDialog({
        id = "add-extension",
        title = I18n:T("settings.extensions.add.title"),
        size = "normal",
        render = function(view)
            return extensions.renderAddDialog({
                I18n = I18n,
                linkEditor = extLinkEditor,
                state = extDialogState,
                onAddLink = startStoreInstall,
                onSelectProfile = function(key: string)
                    extDialogState.selectedKey = key
                    reloadExtCandidates()
                    refreshExtDialog()
                end,
                onOpenProfileMenu = function()
                    Popover:Toggle({
                        id = "ext-browser-profile",
                        anchorKey = "ext-browser-profile-btn",
                        side = "bottom",
                        align = "end",
                        gap = 4,
                    })
                end,
                onAddFromBrowser = startBrowserInstall,
                onToggleDev = function()
                    extDialogState.devOpen = not extDialogState.devOpen
                    refreshExtDialog()
                end,
                onPickFolder = startFolderInstall,
                onToggleSites = function()
                    extDialogState.showAllSites = not extDialogState.showAllSites
                    refreshExtDialog()
                end,
                onApprove = approveExtReview,
                onCancelReview = cancelExtReview,
                onClose = function()
                    Dialog:Close()
                end,
            }, view)
        end,
        onDismiss = function()
            Popover:Close()
            local bAny = Browser :: any
            if type(bAny.ExtCancelReview) == "function" then
                bAny:ExtCancelReview()
            end
            extDialogState.review = nil
            extDialogState.showAllSites = false
            extDialogState.approving = false
            extDialogState.busy = nil
            extDialogState.installingId = nil
            extDialogState.message = nil
            extLinkEditor:SetText("")
        end,
    })

    local function runClearBrowsingData()
        local bAny = Browser :: any
        local histOk = true
        if type(bAny.HistoryClear) == "function" then
            histOk = bAny:HistoryClear() == true
        end
        local keeper = recorder:Get()
        if keeper then
            keeper.clear()
        end
        local prof = profileContract:Get()
        local profOk = true
        if type(prof) == "table" and type(prof.clearData) == "function" then
            profOk = pcall(prof.clearData)
        end
        if not profOk then
            clearNoticeText = I18n:T("settings.failed.clear")
            Toast:Error(clearNoticeText :: string)
        elseif not histOk then
            clearNoticeText = I18n:T("settings.cleared.history")
            Toast:Info(clearNoticeText :: string)
        else
            clearNoticeText = I18n:T("settings.cleared")
            Toast:Info(clearNoticeText :: string)
        end
        refreshSettings()
        refresh()
    end

    ctx:AddDialog({
        id = "clear-data",
        title = I18n:T("settings.clear.confirm.title"),
        render = function(view)
            return settings.renderClearConfirm(I18n, view, function()
                Dialog:Close()
            end, function()
                Dialog:Close()
                runClearBrowsingData()
            end)
        end,
    })

    importDialogHandle = ctx:AddDialog({
        id = "import-history",
        title = I18n:T("settings.import.title"),
        render = function(view)
            local target = importTarget
            if target == nil then
                return nil
            end
            return settings.renderImportDialog(I18n, view, target.label, importFailure, function()
                local bAny = Browser :: any
                local res = if type(bAny.HistoryImportProfileAsync) == "function"
                    then bAny:HistoryImportProfileAsync(target.browser, target.profile)
                    elseif type(bAny.HistoryImportProfile) == "function"
                    then bAny:HistoryImportProfile(target.browser, target.profile)
                    else nil
                local imported = if type(res) == "table" and type(res.imported) == "number" then res.imported else nil
                if imported ~= nil and imported > 0 then
                    -- `site-chooser.tsx:164-176`: a clean import closes the dialog; its row says so.
                    importResults[`{target.browser}\n{target.profile}`] = {
                        text = I18n:T("signins.imported.history.only.from", { browser = target.label }),
                        error = false,
                        live = true,
                    }
                    Dialog:Close()
                elseif imported ~= nil then
                    importFailure = { text = I18n:T("signins.imported.none.from", { browser = target.label }), error = false, live = true }
                else
                    importFailure = { text = I18n:T("signins.failed.read"), error = true, live = true }
                end
                if importDialogHandle then
                    importDialogHandle:Refresh()
                end
                refreshSettings()
            end, function()
                Dialog:Close()
            end)
        end,
        onDismiss = function()
            importTarget = nil
            importFailure = nil
        end,
    })

    local settingsPage = settings.create({
        I18n = I18n,
        preferences = preferences,
        updatePreferences = function(patch)
            updatePreferences(patch)
            refreshSettings()
        end,
        capabilities = settingsCapabilities,
        sitePermissions = sitePermissionsList,
        permissionRestartNote = function(): boolean
            local eng = Browser:Engine() :: any
            return eng ~= nil and eng.state == "ready" and eng.permissionSync ~= true
        end,
        setSitePermission = function(origin, permission, setting)
            setSitePermission(origin, permission, setting)
            refreshSettings()
        end,
        removeSitePermission = function(origin, permission)
            removeSitePermission(origin, permission)
            refreshSettings()
        end,
        clearNotice = function(): string?
            return clearNoticeText
        end,
        requestClearBrowsingData = function()
            Dialog:Open("clear-data")
        end,
        importSources = listImportSources,
        importIcon = function(browserId: string): ImageId?
            -- Each browser's icon is read once a window, and one without an icon is not asked again.
            local known = importIcons[browserId]
            if known ~= nil then
                return if known == false then nil else known :: ImageId
            end
            local bAny = Browser :: any
            local image: ImageId? = nil
            if type(bAny.HistoryImportIcon) == "function" then
                local ok, kind, data, width, height = pcall(function()
                    return bAny:HistoryImportIcon(browserId)
                end)
                if ok and kind == "rgba" then
                    image = Images:FromRgba(data, width, height)
                elseif ok and kind == "png" then
                    image = Images:FromEncoded(data)
                end
            end
            importIcons[browserId] = if image ~= nil then image else false
            return image
        end,
        importMessage = function(browserId: string, profileId: string): settings.ImportMessage?
            return importResults[`{browserId}\n{profileId}`]
        end,
        openImport = function(browserId: string, profileId: string)
            for _, browser in importSources do
                if browser.id == browserId then
                    for _, prof in browser.profiles do
                        if prof.id == profileId then
                            -- `choices.tsx` `choiceLabel`: the profile's name only for a browser with several.
                            importTarget = {
                                browser = browserId,
                                profile = profileId,
                                label = if #browser.profiles > 1 then `{browser.name} · {prof.name}` else browser.name,
                            }
                            importFailure = nil
                            Dialog:Open("import-history")
                            return
                        end
                    end
                end
            end
        end,
        Menu = Menu,
        renderImport = function(view)
            local prof = profileContract:Get()
            if type(prof) == "table" and type(prof.renderImport) == "function" then
                return prof.renderImport(view)
            end
            return nil
        end,
        renderExtensions = function(view)
            local ext = extContract:Get()
            if type(ext) == "table" and type(ext.renderSettings) == "function" then
                return ext.renderSettings(view)
            end
            if not extNativeAvailable() then
                return nil
            end
            local bAny = Browser :: any
            local rawList = if type(bAny.ExtList) == "function" then bAny:ExtList(storedExtensions()) else {}
            local list: { extensions.ProfileExtension } = if type(rawList) == "table" then rawList :: any else {}
            return extensions.renderSettingsSection({
                I18n = I18n,
                available = Browser:Available(),
                busy = extDialogState.busy ~= nil or extDialogState.approving,
                list = list,
                onOpenAdd = openAddExtensionDialog,
                onToggle = function(id: string, enabled: boolean)
                    local res = if type(bAny.ExtSetEnabled) == "function"
                        then bAny:ExtSetEnabled(id, enabled, storedExtensions())
                        else nil
                    if type(res) == "table" and res.ok == true and type(res.records) == "table" then
                        extensionsStore:Set(res.records)
                    else
                        Toast:Error(I18n:T("settings.failed.extension"))
                    end
                    refreshSettings()
                end,
                onRemove = function(id: string)
                    local res = if type(bAny.ExtRemove) == "function"
                        then bAny:ExtRemove(id, storedExtensions())
                        else nil
                    if type(res) == "table" and res.ok == true and type(res.records) == "table" then
                        extensionsStore:Set(res.records)
                    else
                        Toast:Error(I18n:T("settings.failed.extension"))
                    end
                    refreshSettings()
                end,
            }, view)
        end,
        remoteSignInsEnabled = function(): boolean?
            local bAny = Browser :: any
            if type(bAny.ShareSignIns) ~= "function" or bAny:ShareSignIns() == nil then
                return nil
            end
            return preferences().remoteSignIns
        end,
        setRemoteSignIns = function(enabled: boolean)
            -- Kept across restarts as official keeps `remote-sign-ins` (`profile/index.ts:118,657-661`).
            local bAny = Browser :: any
            updatePreferences({ remoteSignIns = enabled })
            bAny:SetShareSignIns(enabled)
            refreshSettings()
        end,
        pickCookieFile = function()
            local prof = profileContract:Get()
            if type(prof) == "table" and type(prof.pickCookieFile) == "function" then
                prof.pickCookieFile()
            end
        end,
    }, refreshSettings)

    -- The host re-sends it to every engine that starts with a cookie store (`#286`), so setting it
    -- once here carries the person's choice across restarts without starting the engine.
    local function syncRemoteSignIns()
        local bAny = Browser :: any
        if type(bAny.SetShareSignIns) == "function" then
            bAny:SetShareSignIns(preferences().remoteSignIns)
        end
    end
    if preferences().remoteSignIns then
        syncRemoteSignIns()
    end
    prefsStore.Changed:Connect(function()
        syncRemoteSignIns()
        refreshSettings()
    end)

    settingsHandle = ctx:AddSettingsPage({
        id = "browser",
        icon = "globe",
        title = I18n:T("settings.title"),
        entries = settings.entries(I18n, settingsCapabilities()),
        render = function(view, props)
            return settingsPage.render(view, props)
        end,
    })

    mcp.add(ctx, Browser, Layout)
end)
