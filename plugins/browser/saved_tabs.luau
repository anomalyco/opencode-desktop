--!strict
-- Saved browser tab metadata for immediate side-strip restore and loading hold matching upstream
-- `UP-100` (`#65067`, `packages/gui-plugins/src/browser/saved-tabs.ts`).

local savedTabs = {}

local MAX_SESSIONS = 50
local MAX_SAVED_ICON = 6000

export type SavedTab = {
    id: string,
    url: string,
    title: string?,
    icon: string?,
}

export type SavedSession = {
    session: string,
    tabs: { SavedTab },
}

export type SavedStore = {
    sessions: { SavedSession },
}

export type Reported = {
    id: string,
    url: string,
    title: string?,
    loading: boolean?,
}

local function site(url: string): string
    local scheme, host = string.match(url, "^([%a][%w+.-]*://)([^/?#]+)")
    if scheme and host then
        local lower = string.lower(scheme)
        if lower == "http://" or lower == "https://" then
            return `{lower}{string.lower(host)}`
        end
    end
    return url
end

function savedTabs.normalize(raw: any): SavedStore
    if type(raw) ~= "table" or type(raw.sessions) ~= "table" then
        return { sessions = {} }
    end
    return raw :: SavedStore
end

function savedTabs.find(saved: SavedStore, session: string, id: string): SavedTab?
    for _, entry in saved.sessions do
        if entry.session == session then
            for _, tab in entry.tabs do
                if tab.id == id then
                    return tab
                end
            end
            return nil
        end
    end
    return nil
end

-- Title drawn on the tab strip: the live page's title, else the saved title while on the saved URL
-- or still loading (redirecting from the saved URL).
function savedTabs.tabTitle(url: string, title: string?, loading: boolean?, saved: SavedTab?): string?
    if title ~= nil and title ~= "" and title ~= "New tab" then
        return title
    end
    if saved ~= nil and saved.title ~= nil and saved.title ~= "" and (saved.url == url or loading == true) then
        return saved.title
    end
    return nil
end

-- Icon drawn on the tab strip: the live page's `data:image/` icon, else the saved icon while on
-- the same site/origin.
function savedTabs.tabIcon(url: string, icon: string?, saved: SavedTab?): string?
    if type(icon) == "string" and string.sub(icon, 1, 11) == "data:image/" then
        return icon
    end
    if saved ~= nil and type(saved.icon) == "string" and string.sub(saved.icon, 1, 11) == "data:image/" and site(saved.url) == site(url) then
        return saved.icon
    end
    return nil
end

local function sameTabs(a: { SavedTab }, b: { SavedTab }): boolean
    if #a ~= #b then
        return false
    end
    for i = 1, #a do
        local x, y = a[i], b[i]
        if x.id ~= y.id or x.url ~= y.url or x.title ~= y.title or x.icon ~= y.icon then
            return false
        end
    end
    return true
end

-- Replaces `session`'s saved tabs with `tabs`, keeping existing saved titles/icons while loading.
-- Returns `nil` when unchanged so callers avoid redundant persistence writes.
function savedTabs.save(saved: SavedStore, session: string, tabs: { Reported }): SavedStore?
    local earlier: { SavedTab } = {}
    for _, entry in saved.sessions do
        if entry.session == session then
            earlier = entry.tabs
            break
        end
    end
    local nextList: { SavedTab } = {}
    for _, tab in tabs do
        local lower = string.lower(tab.url)
        -- Only persist regular HTTP(S) web tabs; never previews, artifacts, or local `file://`.
        if string.match(lower, "^https?://") ~= nil then
            local before: SavedTab? = nil
            for _, item in earlier do
                if item.id == tab.id then
                    before = item
                    break
                end
            end
            local entry: SavedTab = { id = tab.id, url = tab.url }
            local t = savedTabs.tabTitle(tab.url, tab.title, tab.loading, before)
            local ic = savedTabs.tabIcon(tab.url, nil, before)
            if t then
                entry.title = t
            end
            if ic then
                entry.icon = ic
            end
            table.insert(nextList, entry)
        end
    end
    if sameTabs(earlier, nextList) then
        return nil
    end
    local out: { SavedSession } = {}
    if #nextList > 0 then
        table.insert(out, { session = session, tabs = nextList })
    end
    for _, entry in saved.sessions do
        if entry.session ~= session and #out < MAX_SESSIONS then
            local copyTabs: { SavedTab } = {}
            for _, tab in entry.tabs do
                table.insert(copyTabs, table.clone(tab))
            end
            table.insert(out, { session = entry.session, tabs = copyTabs })
        end
    end
    return { sessions = out }
end

-- Records a bounded `data:image/` favicon on an already-saved tab. Returns `nil` when unchanged.
function savedTabs.saveIcon(saved: SavedStore, session: string, id: string, icon: string): SavedStore?
    if string.sub(icon, 1, 11) ~= "data:image/" or #icon > MAX_SAVED_ICON then
        return nil
    end
    local current = savedTabs.find(saved, session, id)
    if current == nil or current.icon == icon then
        return nil
    end
    local out: { SavedSession } = {}
    for _, entry in saved.sessions do
        local copyTabs: { SavedTab } = {}
        for _, tab in entry.tabs do
            local copy = table.clone(tab)
            if entry.session == session and tab.id == id then
                copy.icon = icon
            end
            table.insert(copyTabs, copy)
        end
        table.insert(out, { session = entry.session, tabs = copyTabs })
    end
    return { sessions = out }
end

return savedTabs
