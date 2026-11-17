--!strict
-- Desktop's built-in browser plugin (`packages/gui-plugins/src/browser`): one side-region tab per
-- page of the session (`browser:<page id>`), titled by its page; the pane with its bar, address
-- field, options menu and new tab page (`panel`); Open browser (Ctrl+Alt+B, ^?B on macOS) and the
-- pane's own commands; the + menu's and New tab's Browser row; the element picker's notes in the
-- composer; the history the app half keeps; and the agent's `browser_*` tools (`mcp`). The pages
-- themselves are the host's browser engine's (`Browser`, `ui.browserSurface`).

local plugin = require("@opencode/plugin")
local address = require("@self/address")
local history = require("@self/history")
local mcp = require("@self/mcp")
local panel = require("@self/panel")
local site = require("@self/site")

-- The panel's id: its tabs key as `browser:<page id>`, as Desktop's.
local PANEL = "main"

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Sessions = ctx:GetService("Sessions")
    local Browser = ctx:GetService("Browser")
    local Layout = ctx:GetService("Layout")
    local App = ctx:GetService("App")
    local Store = ctx:GetService("Store")
    local Contracts = ctx:GetService("Contracts")
    local Menu = ctx:GetService("Menu")
    local System = ctx:GetService("System")
    local Toast = ctx:GetService("Toast")
    local Composer = ctx:GetService("Composer")
    local Server = ctx:GetService("Server")
    local Servers = ctx:GetService("Servers")
    local Popover = ctx:GetService("Popover")

    -- The window's address field: it edits one page's address at a time.
    local editor = ctx:CreateEditor("")
    local stored = Store:Value("history", { visits = {} })
    local recorder = Contracts:Use("browser.history")

    local state = address.initial()
    local epochs: { [string]: address.Epoch } = {}
    -- Each page's last recorded visit, so a report that repeats it doesn't write the history.
    local recorded: { [string]: string } = {}
    local hovered = false
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

    local function pageUrl(tab: BrowserTab): string
        local url = tab:Url()
        return if url == "about:blank" then "" else url
    end

    -- Page `tab`'s report counts, folded with what it reports now.
    local function epoch(tab: BrowserTab): address.Epoch
        local e = address.fold(epochs[tab.Id], pageUrl(tab), tab:Loading())
        epochs[tab.Id] = e
        return e
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
        -- What the field was given (an address put in it) isn't typing.
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

    -- A page that moved: its counts, the history once it loaded (`record`), and the pane.
    Browser.Changed:Connect(function(id: string)
        local tab = Browser:Get(id)
        if tab == nil then
            epochs[id] = nil
            recorded[id] = nil
            refresh()
            return
        end
        epoch(tab)
        local url = pageUrl(tab)
        if not tab:Loading() and history.recordable(url) then
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
            return Server:IsLocal()
        end
        if lower ~= "http://" and lower ~= "https://" then
            return false
        end
        local name = string.match(host, "^%[[^%]]*%]") or string.match(host, "^[^:]*") or host
        return Server:IsLocal() or not address.loopback(name)
    end

    -- Where the keyboard goes when a new blank tab opens: its address field, or where it was
    -- (Open browser's shortcut pressed outside the side region: Desktop's chat keeps it).
    type Keyboard = "address" | "kept"
    -- Blank tabs that opened with the keyboard kept, until their first focus.
    local kept: { [string]: boolean } = {}
    local opening: Keyboard? = nil

    -- A new blank tab in the session (the routed one by default).
    local function openNew(sid: string?, keyboard: Keyboard)
        local session = if sid then Sessions:Get(sid) else Sessions:Routed()
        if session == nil or not Browser:Available() then
            return
        end
        opening = keyboard
        -- `background` opens and selects the tab and leaves the keyboard (and the strip's focus) where it was.
        local tab = Browser:OpenAsync({ url = "about:blank", sessionId = session.id, background = keyboard == "kept" })
        opening = nil
        if tab and keyboard == "address" then
            edit(tab)
        elseif tab then
            kept[tab.Id] = true
        end
    end

    -- Site information: the page it describes, and its cookie count's machine.
    local siteState: site.State = site.initial
    local sitePage: { tab: string, url: string }? = nil

    local function siteDispatch(event: site.Event)
        local nextState, effects = site.step(siteState, event)
        siteState = nextState
        for _, effect in effects do
            local described = sitePage
            if effect.type == "notify-clear-failed" then
                Toast:Info(I18n:T("common.requestFailed"))
            elseif described then
                local attempt = effect.attempt
                local tab = Browser:Get(described.tab)
                if effect.type == "load" then
                    task.delay(site.LOAD_TIMEOUT, function()
                        siteDispatch({ type = "timeout", phase = "loading", attempt = attempt })
                    end)
                    task.spawn(function()
                        local ok, answer = pcall(function()
                            return if tab then tab:DevtoolsAsync("Network.getCookies", { urls = { described.url } }) else nil
                        end)
                        local cookies = if ok and type(answer) == "table" and type(answer.cookies) == "table" then answer.cookies else nil
                        siteDispatch(if cookies then { type = "load.succeeded", attempt = attempt, cookies = #cookies } else { type = "load.failed", attempt = attempt })
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
                            local answer = tab:DevtoolsAsync("Network.getCookies", { urls = { described.url } })
                            for _, cookie in (if type(answer) == "table" and type(answer.cookies) == "table" then answer.cookies else {}) do
                                tab:DevtoolsAsync("Network.deleteCookies", { name = cookie.name, domain = cookie.domain, path = cookie.path })
                            end
                            tab:Reload()
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
    }

    ctx:AddPopover({
        id = "site",
        title = I18n:T("site.info"),
        render = function(view, props)
            local data: { [string]: any } = props.data or {}
            return site.render(siteEnv, view, if type(data.url) == "string" then data.url else "")
        end,
    })

    local function options(tab: BrowserTab)
        local url = pageUrl(tab)
        local zoomed = url ~= "" and math.abs(tab:Zoom() - 1) > 0.001
        -- Zooming closes the menu; the address field shows any zoom but 100% afterwards.
        local picked = Menu:OpenAsync({
            anchorKey = "browser-options",
            label = I18n:T("options"),
            align = "end",
            width = 172,
            items = {
                { id = "zoom.in", title = I18n:T("zoom.in"), keys = "mod+plus", enabled = url ~= "" },
                { id = "zoom.out", title = I18n:T("zoom.out"), keys = "mod+-", enabled = url ~= "" },
                { id = "zoom.reset", title = I18n:T("zoom.reset"), enabled = zoomed },
                { kind = "separator" },
                { id = "link.copy", title = I18n:T("link.copy"), enabled = url ~= "" },
                { id = "external", title = I18n:T("external.menu"), enabled = reachable(url) },
                { kind = "separator" },
                { id = "history.clear", title = I18n:T("history.clear"), enabled = #visits() > 0 },
            },
        })
        if picked == "zoom.in" then
            tab:SetZoom("in")
        elseif picked == "zoom.out" then
            tab:SetZoom("out")
        elseif picked == "zoom.reset" then
            tab:SetZoom("reset")
        elseif picked == "link.copy" then
            System:CopyText(url)
            Toast:Info(I18n:T("common.copied"))
        elseif picked == "external" then
            System:OpenExternal(url)
        elseif picked == "history.clear" then
            local keeper = recorder:Get()
            if keeper then
                keeper.clear()
            end
        end
    end

    -- What the pane's controls do.
    local function act(what: string, tab: BrowserTab?, arg: any?)
        if what == "hover" then
            if hovered ~= (arg == true) then
                hovered = arg == true
                refresh()
            end
            return
        end
        if tab == nil then
            return
        end
        if what == "edit" then
            edit(tab)
        elseif what == "go" then
            dispatch({ type = "navigated", tab = tab.Id, url = arg :: string, epoch = epoch(tab) })
        elseif what == "zoom" then
            tab:SetZoom(arg :: any)
        elseif what == "page" then
            local url = pageUrl(tab)
            local _, host = address.parts(url)
            local title = tab:Title()
            Composer:InsertPill(`browser:{url}`, `{if title ~= "" then title else host} ({url})`)
        elseif what == "external" then
            System:OpenExternal(pageUrl(tab))
        elseif what == "options" then
            options(tab)
        elseif what == "site" then
            -- Site information describes one page: another page or address reads anew.
            local url = pageUrl(tab)
            if sitePage == nil or sitePage.tab ~= tab.Id or sitePage.url ~= url then
                siteState = site.initial
            end
            sitePage = { tab = tab.Id, url = url }
            if not Popover:IsOpen("site") then
                siteDispatch({ type = "opened" })
            end
            Popover:Toggle({ id = "site", anchorKey = "browser-site-anchor", side = "bottom", align = "start", gap = 8, data = { url = url, tab = tab.Id } })
        end
    end

    local env: panel.Env = {
        I18n = I18n,
        Browser = Browser,
        Server = Server,
        menuTrigger = App.Platform ~= "macos",
        editor = editor,
        state = function()
            return state
        end,
        epoch = epoch,
        visits = visits,
        hovered = function()
            return hovered
        end,
        reachable = reachable,
        siteOpen = function()
            return Popover:IsOpen("site")
        end,
        act = act,
    }

    -- A tab's label: "New tab" for a blank page, else the page's title, else its address.
    local function label(info: BrowserTabInfo): string
        if info.url == "" then
            return I18n:T("tab.new")
        end
        if info.title ~= "" and info.title ~= "New tab" then
            return info.title
        end
        return info.url
    end

    panelHandle = ctx:AddPanel({
        id = PANEL,
        title = I18n:T("tab.title"),
        icon = "globe",
        launcher = false,
        -- A tab whose page is gone (closed here or by the engine) leaves the strip.
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
        render = function(view, props)
            return panel.render(env, view, props)
        end,
        -- A blank tab starts in its address field, like a browser's new tab.
        onFocus = function(props)
            local tab = if props.instance then Browser:Get(props.instance) else nil
            if tab == nil or opening == "kept" or kept[tab.Id] then
                if tab then
                    kept[tab.Id] = nil
                end
                return
            end
            if props.restored ~= true and pageUrl(tab) == "" then
                edit(tab)
            end
        end,
        -- Closing a page's tab closes the page.
        onClose = function(props)
            local tab = if props.instance then Browser:Get(props.instance) else nil
            if tab then
                tab:Close()
            end
        end,
        -- A blank new tab does not come back; a page comes back as a new tab on its address.
        closedTab = function(props): ClosedTab?
            local tab = if props.instance then Browser:Get(props.instance) else nil
            local url = if tab then pageUrl(tab) else ""
            if url == "" then
                return nil
            end
            local _, host = address.parts(url)
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
            -- Desktop's shortcut focuses the new tab's address only from inside the side region; from
            -- the chat (its composer or timeline) the keyboard stays there (measured with real input).
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

    -- The pane's own keys, as a browser's: each answers only with the keyboard in the pane and
    -- leaves the key to the app elsewhere (Ctrl+L focuses the composer, Ctrl+T opens a session,
    -- Ctrl+W closes a tab, Ctrl+Shift+C copies in the terminal).
    local function scoped(id: string, bind: string, hidden: boolean, title: string, act: (tab: BrowserTab) -> boolean)
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
                if tab == nil or not act(tab) then
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
        -- The pointer on the row comes 150-400 ms before its click: the engine starts then.
        hover = function()
            Browser:Prepare()
        end,
        run = function(sid: string?)
            openNew(sid, "address")
        end,
    })

    mcp.add(ctx, Browser, Layout)
end)
