--!strict
-- The browser's address field (address.ts, address-machine.ts). At rest it shows the page's URL;
-- editing shows the user's text and suggestions; a submitted address stays in the field until
-- the page reports where it went. The page's reports are counted (`fold`), and a submit keeps
-- the counts it saw, so the next URL change or finished load ends the hold, while focusing and
-- leaving the field in between keeps it.
--
-- One machine per window: the window has one address editor, which edits one page at a time;
-- each page keeps its own hold.

local history = require("./history")

local address = {}

-- Suggestions the address field lists.
address.SUGGESTIONS = 5
-- Long text scrolls the input, which the inline hint can't follow, so only a query this short
-- shows it.
address.HINT_MAX_LENGTH = 48

-- A page's reports, counted: each count moves on every report, even one that repeats a value.
export type Epoch = {
    url: string,
    loading: boolean,
    -- Finished loads.
    settled: number,
    -- URL changes and finished loads.
    followed: number,
    -- Any reported movement.
    moved: number,
}

-- A submitted address, and the counts when it was submitted.
export type Hold = { text: string, followed: number, moved: number }

export type Edit = { status: "rest" } | {
    status: "editing",
    -- The page whose address is edited.
    tab: string,
    text: string,
    -- The user typed since focusing; suggestions follow typing, not a focused URL.
    typed: boolean,
    -- The suggestion Enter opens, from 1; 0 opens the typed text.
    active: number,
    -- Escape closed the suggestions; typing opens them again.
    dismissed: boolean,
}

export type State = { edit: Edit, holds: { [string]: Hold } }

export type Event =
    { type: "focused", tab: string, epoch: Epoch }
    | { type: "blurred" }
    | { type: "input", value: string, deleting: boolean, visits: { history.Visit } }
    | { type: "key", key: string, visits: { history.Visit } }
    | { type: "submitted", tab: string, epoch: Epoch, visits: { history.Visit } }
    | { type: "navigated", tab: string, url: string, epoch: Epoch }

export type Effect = { type: "select" } | { type: "blur" } | { type: "navigate", tab: string, url: string }

function address.initial(): State
    return { edit = { status = "rest" }, holds = {} }
end

local SELECT: Effect = { type = "select" }
local BLUR: Effect = { type = "blur" }

-- The address as the field draws it at rest: the whole URL, its host emphasized. The parts join
-- back to exactly the text given.
function address.parts(url: string): (string, string, string)
    local scheme = string.match(url, "^[%a][%w+.-]*://") or ""
    local rest = string.sub(url, #scheme + 1)
    local host = string.match(rest, "^[^/?#]*") or rest
    return scheme, host, string.sub(rest, #host + 1)
end

-- Whether a host names this machine.
function address.loopback(host: string): boolean
    local h = string.lower(host)
    return h == "localhost" or h == "[::1]" or h == "::1" or string.match(h, "^127%.%d+%.%d+%.%d+$") ~= nil or string.sub(h, -10) == ".localhost"
end

-- Whether the address field would search for the text rather than open it: text with spaces,
-- or that names no host a page opens on (`localhost`, a port, a dotted host).
function address.searches(input: string): boolean
    local value = string.gsub(input, "^%s*(.-)%s*$", "%1")
    if value == "" or string.match(string.lower(value), "^about:%S*$") then
        return false
    end
    if string.match(value, "^[%a][%w+.-]*://") then
        return string.match(value, "^[%a][%w+.-]*://[^/?#%s]+") == nil
    end
    if string.find(value, "%s") then
        return true
    end
    local host = string.match(value, "^[^/?#]*") or ""
    local named = string.match(string.lower(host), "^localhost:?%d*$") ~= nil or string.match(host, "^%[[%x:.]+%]:?%d*$") ~= nil or string.match(host, ":%d+$") ~= nil or string.match(host, "^[^.]+%.[^.]") ~= nil
    return not named
end

-- Folds a page report into its counts.
function address.fold(previous: Epoch?, url: string, loading: boolean): Epoch
    if previous == nil then
        return { url = url, loading = loading, settled = 0, followed = 0, moved = 0 }
    end
    local settled = if previous.loading and not loading then previous.settled + 1 else previous.settled
    local followed = if url ~= previous.url or settled ~= previous.settled then previous.followed + 1 else previous.followed
    local moved = if url ~= previous.url or loading ~= previous.loading then previous.moved + 1 else previous.moved
    return { url = url, loading = loading, settled = settled, followed = followed, moved = moved }
end

-- The hold of page `tab`, while the page has reported no URL change or finished load since.
local function activeHold(state: State, tab: string, epoch: Epoch): Hold?
    local hold = state.holds[tab]
    if hold and hold.followed == epoch.followed then
        return hold
    end
    return nil
end

local function suggestions(state: State, visits: { history.Visit }): { history.Visit }
    local edit = state.edit
    if edit.status == "editing" and edit.typed then
        return history.suggest(visits, edit.text, address.SUGGESTIONS)
    end
    return {}
end

local function listed(state: State, found: { history.Visit }): boolean
    local edit = state.edit
    return edit.status == "editing" and #found > 0 and not edit.dismissed
end

-- The text in page `tab`'s field: the user's while editing it, a submitted address until the
-- page reports, otherwise the page's URL.
local function field(state: State, tab: string, epoch: Epoch): string
    local edit = state.edit
    if edit.status == "editing" and edit.tab == tab then
        return edit.text
    end
    local hold = activeHold(state, tab, epoch)
    return if hold then hold.text else epoch.url
end

local function navigate(state: State, tab: string, url: string, epoch: Epoch): (State, { Effect })
    local holds = table.clone(state.holds)
    -- A blank page keeps the field empty.
    holds[tab] = { text = if url == "about:blank" then "" else url, followed = epoch.followed, moved = epoch.moved }
    return { edit = { status = "rest" }, holds = holds }, { { type = "navigate", tab = tab, url = url }, BLUR }
end

function address.step(state: State, event: Event): (State, { Effect })
    local edit = state.edit
    if event.type == "focused" then
        if edit.status == "editing" and edit.tab == event.tab then
            return state, {}
        end
        local hold = activeHold(state, event.tab, event.epoch)
        local text = if hold then hold.text else event.epoch.url
        return { edit = { status = "editing", tab = event.tab, text = text, typed = false, active = 0, dismissed = false }, holds = state.holds }, { SELECT }
    elseif event.type == "blurred" then
        -- Leaving the field without submitting cancels the edit; a submitted address not reported
        -- yet stays (its hold).
        if edit.status ~= "editing" then
            return state, {}
        end
        return { edit = { status = "rest" }, holds = state.holds }, {}
    elseif event.type == "input" then
        if edit.status ~= "editing" then
            return state, {}
        end
        local first = history.suggest(event.visits, event.value, 1)[1]
        -- Typing completes the first suggestion, as a search would; deleting never does.
        local complete = first ~= nil and not event.deleting and (history.completes(first, event.value) or address.searches(event.value))
        return { edit = { status = "editing", tab = edit.tab, text = event.value, typed = true, active = if complete then 1 else 0, dismissed = false }, holds = state.holds }, {}
    elseif event.type == "key" then
        if edit.status ~= "editing" then
            return state, {}
        end
        local found = suggestions(state, event.visits)
        if event.key == "Escape" then
            if listed(state, found) then
                return { edit = { status = "editing", tab = edit.tab, text = edit.text, typed = edit.typed, active = 0, dismissed = true }, holds = state.holds }, {}
            end
            return state, { BLUR }
        end
        local count = #found
        if count == 0 then
            return state, {}
        end
        -- Desktop's index runs from -1 (the typed text) to count - 1 and wraps.
        local at = edit.active - 1
        local by = if event.key == "ArrowDown" then 1 else -1
        local nextAt = if at + by < -1 then count - 1 else ((at + by + 1) % (count + 1)) - 1
        return { edit = { status = "editing", tab = edit.tab, text = edit.text, typed = edit.typed, active = nextAt + 1, dismissed = false }, holds = state.holds }, {}
    elseif event.type == "submitted" then
        local found = suggestions(state, event.visits)
        local picked = if edit.status == "editing" and listed(state, found) then found[edit.active] else nil
        if picked then
            return navigate(state, event.tab, picked.url, event.epoch)
        end
        local text = string.gsub(field(state, event.tab, event.epoch), "^%s*(.-)%s*$", "%1")
        if text ~= "" then
            return navigate(state, event.tab, text, event.epoch)
        end
        return state, { BLUR }
    else
        return navigate(state, event.tab, event.url, event.epoch)
    end
end

export type View = {
    -- The field edits this page.
    editing: boolean,
    field: string,
    -- What the user typed, which suggestions highlight; empty at rest.
    query: string,
    typed: boolean,
    -- The suggestions show.
    listed: boolean,
    suggestions: { history.Visit },
    -- The suggestion Enter opens, from 1; 0 for none.
    active: number,
    chosen: history.Visit?,
    -- A submitted navigation the page hasn't reported yet; keeps the empty state hidden.
    navigating: boolean,
}

-- What page `tab`'s address field draws.
function address.view(state: State, tab: string, epoch: Epoch, visits: { history.Visit }): View
    local edit = state.edit
    local editing = edit.status == "editing" and edit.tab == tab
    local found = if editing then suggestions(state, visits) else {}
    local shown = editing and listed(state, found)
    local active = if shown and edit.status == "editing" then edit.active else 0
    local hold = state.holds[tab]
    return {
        editing = editing,
        field = field(state, tab, epoch),
        query = if editing and edit.status == "editing" then edit.text else "",
        typed = editing and edit.status == "editing" and edit.typed,
        listed = shown,
        suggestions = found,
        active = active,
        chosen = if active > 0 then found[active] else nil,
        navigating = hold ~= nil and hold.moved == epoch.moved,
    }
end

return address
