--!strict
-- The pages the browser showed (history.ts): newest first, one list for every session, kept on
-- this device. The new tab page lists the first few; the address field's suggestions search it.

local history = {}

export type Visit = { url: string, title: string }

-- Visits the history keeps.
local MAX_VISITS = 30

-- The pages the history records: web pages and workspace files, not blank or internal ones.
function history.recordable(url: string): boolean
    local lower = string.lower(url)
    return string.match(lower, "^https?://") ~= nil or string.match(lower, "^file://") ~= nil
end

-- Puts a visit first, replacing an earlier visit of the same URL, and drops the oldest past the
-- limit. A visit without a title yet keeps the earlier visit's.
function history.remember(visits: { Visit }, visit: Visit): { Visit }
    local title = visit.title
    for _, v in visits do
        if v.url == visit.url and title == "" then
            title = v.title
        end
    end
    local out: { Visit } = { { url = visit.url, title = title } }
    for _, v in visits do
        if v.url ~= visit.url and #out < MAX_VISITS then
            table.insert(out, { url = v.url, title = v.title })
        end
    end
    return out
end

-- The address without its web scheme or a leading `www.`, as people type it.
function history.bare(url: string): string
    local s = string.gsub(url, "^[Hh][Tt][Tt][Pp][Ss]?://", "")
    s = string.gsub(s, "^[Ww][Ww][Ww]%.", "")
    return s
end

local function trimmed(query: string): string
    return string.lower((string.gsub(query, "^%s*(.-)%s*$", "%1")))
end

-- Whether the visit's address begins with what the user typed, so Enter may open it in place of
-- the typed text.
function history.completes(visit: Visit, query: string): boolean
    local needle = trimmed(query)
    return needle ~= "" and string.sub(string.lower(history.bare(visit.url)), 1, #needle) == needle
end

-- Visits whose address or title contains the query, case-insensitively. Addresses that begin
-- with the query come first, as the address field completes them; within each group the newest
-- visit leads.
function history.suggest(visits: { Visit }, query: string, limit: number): { Visit }
    local needle = trimmed(query)
    if needle == "" then
        return {}
    end
    local first: { Visit } = {}
    local rest: { Visit } = {}
    for _, v in visits do
        local url = string.lower(history.bare(v.url))
        if string.find(url, needle, 1, true) or string.find(string.lower(v.title), needle, 1, true) then
            table.insert(if string.sub(url, 1, #needle) == needle then first else rest, v)
        end
    end
    local out: { Visit } = {}
    for _, list in { first, rest } do
        for _, v in list do
            if #out < limit then
                table.insert(out, v)
            end
        end
    end
    return out
end

export type Part = { text: string, match: boolean }

-- Splits text around case-insensitive matches of the query, for drawing the matched parts.
function history.highlight(text: string, query: string): { Part }
    local needle = trimmed(query)
    if needle == "" then
        return { { text = text, match = false } }
    end
    local lower = string.lower(text)
    local out: { Part } = {}
    local at = 1
    while true do
        local s, e = string.find(lower, needle, at, true)
        if s == nil or e == nil then
            break
        end
        if s > at then
            table.insert(out, { text = string.sub(text, at, s - 1), match = false })
        end
        table.insert(out, { text = string.sub(text, s, e), match = true })
        at = e + 1
    end
    if at <= #text then
        table.insert(out, { text = string.sub(text, at), match = false })
    end
    return out
end

return history
