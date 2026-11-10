--!strict
-- The changes a local or beta build carries (`beta-notes.ts`): the build's commit, its newest
-- commits, and the feature and fix notes among them, as `App.BetaNotes` hands them over.

local notes = {}

-- The changes lists at most this many and links the rest.
local SHOWN = 10

local REPOSITORY = "https://github.com/anomalyco/rune-desktop"

export type Note = {
    sha: string,
    type: string,
    summary: string,
    pr: { number: number, url: string }?,
    author: { name: string, avatar: string? }?,
}

export type Notes = { added: { Note }, fixed: { Note }, total: number, more: boolean }

export type Embedded = { sha: string, commits: { string }, notes: { any } }

-- The embedded notes, or nil when the build carries none.
function notes.embedded(value: any): Embedded?
    if type(value) ~= "table" or type(value.sha) ~= "string" or type(value.commits) ~= "table" or type(value.notes) ~= "table" then
        return nil
    end
    return value :: Embedded
end

local function item(raw: any): Note
    local summary = string.gsub(string.gsub(tostring(raw.summary or ""), "^%s+", ""), "%s+$", "")
    summary = string.gsub(summary, "%.$", "")
    summary = string.upper(string.sub(summary, 1, 1)) .. string.sub(summary, 2)
    local pr = if type(raw.pr) == "number" then { number = raw.pr, url = `{REPOSITORY}/pull/{raw.pr}` } else nil
    local author = if type(raw.author) == "table" and type(raw.author.name) == "string" then { name = raw.author.name, avatar = raw.author.avatar } else nil
    return { sha = tostring(raw.sha or ""), type = tostring(raw.type or "fix"), summary = summary, pr = pr, author = author }
end

-- Every change the build carries, features first, then fixes, newest first: the What's new page.
function notes.all(embedded: Embedded): Notes
    local added, fixed = {}, {}
    for _, raw in embedded.notes do
        table.insert(if raw.type == "feat" then added else fixed, item(raw))
    end
    return { added = added, fixed = fixed, total = #embedded.notes, more = false }
end

-- The changes since the build at `previous`: the notes of the commits newer than it, features
-- first, then fixes, at most 10 in all and at least half of them features when there are
-- enough. When `previous` is older than the embedded history, the newest notes stand in, with
-- the rest on the What's new page. Nil without a previous commit, or without a change.
function notes.since(embedded: Embedded, previous: string?): Notes?
    if not previous or previous == embedded.sha then
        return nil
    end
    local cut: number? = nil
    local position: { [string]: number } = {}
    for index, commit in embedded.commits do
        position[commit] = index
        if not cut and string.sub(previous, 1, #commit) == commit then
            cut = index
        end
    end
    local list = {}
    for _, raw in embedded.notes do
        if not cut or (position[raw.sha] or cut) < cut then
            table.insert(list, raw)
        end
    end
    if #list == 0 then
        return nil
    end
    local features, fixes = {}, {}
    for _, raw in list do
        table.insert(if raw.type == "feat" then features else fixes, raw)
    end
    local added, fixed = {}, {}
    for i = 1, math.min(#features, math.max(SHOWN // 2, SHOWN - #fixes)) do
        table.insert(added, item(features[i]))
    end
    for i = 1, math.min(#fixes, SHOWN - #added) do
        table.insert(fixed, item(fixes[i]))
    end
    local listed = #added + #fixed
    return { added = added, fixed = fixed, total = if cut then #list else listed, more = not cut or listed < #list }
end

-- The build's commit history on GitHub.
function notes.history(embedded: Embedded): string
    return `{REPOSITORY}/commits/{embedded.sha}`
end

-- The beta run number of a version such as `0.0.0-beta-21180`, or the version itself.
function notes.build(version: string): string
    return string.match(version, "^v?0%.0%.0%-beta%-(%d+[%.%d]*)$") or string.match(version, "^v?0%.0%.0%-next%-(%d+[%.%d]*)$") or version
end

return notes
