--!strict
-- Browsing history helpers, omnibox suggestion merging, completion suffix, and multi-word
-- match highlighting matching upstream `UP-106` (`#65073`,
-- `packages/gui-plugins/src/browser/history/{page.ts,match.ts,suggest.ts}`).

local history = {}

export type Visit = { url: string, title: string }
export type HistoryPage = { url: string, title: string, score: number }
export type Suggestion = { url: string, title: string, completion: string? }
export type Part = { text: string, match: boolean }

local MAX_VISITS = 30
local MAX_URL_LENGTH = 2048

-- The pages the history records: web pages only, never blank pages, internal URLs, or isolated
-- workspace/artifact previews (`file://`, `workspace://`, `artifact://`, `oc-preview://`).
function history.recordable(url: string): boolean
    local lower = string.lower(url)
    return string.match(lower, "^https?://") ~= nil
end

-- Canonical web-page deduplication key matching Rust `history::page::page_key`.
function history.pageKey(url: string): string?
    if #url == 0 or #url > MAX_URL_LENGTH then
        return nil
    end
    local rest = string.match(url, "^[Hh][Tt][Tt][Pp][Ss]?://(.+)$")
    if rest == nil then
        return nil
    end
    local auth = string.match(rest, "^([^/?#]*)") or ""
    if auth == "" or string.find(auth, "@", 1, true) ~= nil then
        return nil
    end
    local host = string.gsub(string.lower(auth), "^www%.", "")
    local tail = string.sub(rest, #auth + 1)
    local pathPart, qfPart = string.match(tail, "^([^?#]*)(.*)$")
    local cleanPath = string.gsub(pathPart or "", "/+$", "")
    return `{host}{cleanPath}{qfPart or ""}`
end

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

function history.forget(visits: { Visit }, url: string): { Visit }
    local targetKey = history.pageKey(url)
    local out: { Visit } = {}
    for _, v in visits do
        local k = history.pageKey(v.url)
        if v.url ~= url and (targetKey == nil or k ~= targetKey) then
            table.insert(out, v)
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

-- Strips trailing slash on a bare host (`https://github.com/` -> `https://github.com`).
local function hostOnly(url: string): string
    return (string.gsub(url, "^([%a][%w+.-]*://[^/?#]+)/$", "%1"))
end

-- What a chosen history row fills into the address field (`page.ts::filled`):
-- strips `http://` and trailing slash on a bare host; keeps `https://`.
function history.filled(url: string): string
    return (string.gsub(hostOnly(url), "^[Hh][Tt][Tt][Pp]://", ""))
end

-- Inline completion suffix after `text`, when `url` begins with `text` (`page.ts::completion`).
-- Returns `""` when `text` is the whole address, `nil` when `url` doesn't begin with `text` or
-- `text` contains spaces.
function history.completion(text: string, url: string): string?
    if text == "" or string.find(text, "%s") ~= nil then
        return nil
    end
    local typed = string.lower(text)
    local hOnly = hostOnly(url)
    local forms = {
        history.bare(hOnly),
        (string.gsub(hOnly, "^[Hh][Tt][Tt][Pp][Ss]?://", "")),
        hOnly,
        history.bare(url),
        (string.gsub(url, "^[Hh][Tt][Tt][Pp][Ss]?://", "")),
        url,
    }
    for _, form in forms do
        if string.sub(string.lower(form), 1, #typed) == typed then
            return string.sub(form, #text + 1)
        end
    end
    return nil
end

local function queryWords(query: string): { string }
    local out: { string } = {}
    for word in string.gmatch(string.lower(query), "%S+") do
        local b = history.bare(word)
        if b ~= "" then
            table.insert(out, b)
        end
    end
    return out
end

-- Quality matching `match.ts::matchQuality`: 2 = all words at word start, 1 = inside, 0 = miss.
local function matchQuality(words: { string }, url: string, title: string): number
    if #words == 0 then
        return 0
    end
    local hay = string.lower(`{history.bare(url)}\n{title}`)
    local quality = 2
    for _, w in words do
        local s = string.find(hay, w, 1, true)
        if s == nil then
            return 0
        end
        local atStart = false
        local pos = s
        while pos ~= nil do
            if pos == 1 then
                atStart = true
                break
            end
            local prev = string.sub(hay, pos - 1, pos - 1)
            if string.match(prev, "[%w]") == nil then
                atStart = true
                break
            end
            pos = string.find(hay, w, pos + 1, true)
        end
        if not atStart then
            quality = 1
        end
    end
    return quality
end

-- Merges SQLite `answer` pages (frecency-scored) with the window's recent `visits` list
-- (`suggest.ts::suggestPages`), returning up to `limit` suggestions with completion suffixes.
function history.suggestMerged(text: string, visits: { Visit }, answer: { HistoryPage }, limit: number): { Suggestion }
    local words = queryWords(text)
    if #words == 0 then
        return {}
    end
    local leadTrimmed = string.gsub(text, "^%s+", "")
    local listed: { [string]: Visit } = {}
    for _, v in visits do
        local k = history.pageKey(v.url) or v.url
        if listed[k] == nil then
            listed[k] = v
        end
    end

    local answered: { [string]: boolean } = {}
    type Cand = { visit: Visit, quality: number, score: number }
    local cands: { Cand } = {}

    for _, page in answer do
        local k = history.pageKey(page.url) or page.url
        answered[k] = true
        local known = listed[k]
        local v: Visit = {
            url = page.url,
            title = if page.title ~= "" then page.title elseif known then known.title else "",
        }
        local q = matchQuality(words, v.url, v.title)
        if q > 0 then
            table.insert(cands, { visit = v, quality = q, score = page.score })
        end
    end

    for idx, v in visits do
        local k = history.pageKey(v.url) or v.url
        if not answered[k] then
            local q = matchQuality(words, v.url, v.title)
            if q > 0 then
                table.insert(cands, { visit = v, quality = q, score = 1 - (idx - 1) / 1000 })
            end
        end
    end

    table.sort(cands, function(a, b)
        if a.quality ~= b.quality then
            return a.quality > b.quality
        end
        return a.score > b.score
    end)

    local out: { Suggestion } = {}
    for i = 1, math.min(limit, #cands) do
        local v = cands[i].visit
        table.insert(out, {
            url = v.url,
            title = v.title,
            completion = history.completion(leadTrimmed, v.url),
        })
    end
    return out
end

-- Legacy single-list suggestion helper kept for callers without a SQLite answer.
function history.suggest(visits: { Visit }, query: string, limit: number): { Visit }
    local merged = history.suggestMerged(query, visits, {}, limit)
    local out: { Visit } = {}
    for _, s in merged do
        table.insert(out, { url = s.url, title = s.title })
    end
    return out
end

function history.completes(visit: Visit, query: string): boolean
    return history.completion(string.gsub(query, "^%s+", ""), visit.url) ~= nil
end

-- Splits `text` around all case-insensitive occurrences of the query's words (`match.ts::markMatches`).
function history.highlight(text: string, query: string): { Part }
    local words = queryWords(query)
    if #words == 0 or text == "" then
        return { { text = text, match = false } }
    end
    local lower = string.lower(text)
    local ranges: { { number } } = {}
    for _, w in words do
        local at = 1
        while true do
            local s, e = string.find(lower, w, at, true)
            if s == nil or e == nil then
                break
            end
            table.insert(ranges, { s, e })
            at = e + 1
        end
    end
    if #ranges == 0 then
        return { { text = text, match = false } }
    end
    table.sort(ranges, function(a, b)
        return a[1] < b[1]
    end)
    local merged: { { number } } = {}
    for _, r in ranges do
        local last = merged[#merged]
        if last == nil or r[1] > last[2] + 1 then
            table.insert(merged, { r[1], r[2] })
        elseif r[2] > last[2] then
            last[2] = r[2]
        end
    end
    local out: { Part } = {}
    local cur = 1
    for _, r in merged do
        if r[1] > cur then
            table.insert(out, { text = string.sub(text, cur, r[1] - 1), match = false })
        end
        table.insert(out, { text = string.sub(text, r[1], r[2]), match = true })
        cur = r[2] + 1
    end
    if cur <= #text then
        table.insert(out, { text = string.sub(text, cur), match = false })
    end
    return out
end

return history
