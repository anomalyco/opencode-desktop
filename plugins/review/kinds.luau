--!strict
-- The review's path helpers (kinds.ts): each changed file's and folder's change kind, and the
-- depth-first order with folders before their sibling files that the change tree and the file
-- arrows follow.

local kinds = {}

-- Each changed file's kind (`add`, `del`, `mix`) and each folder's above it: one kind while all
-- its files changed the same way, else `mix` (`reviewDiffKinds`).
function kinds.of(files: { DiffFile }): { [string]: ChangeKind }
    local out: { [string]: ChangeKind } = {}
    for _, f in files do
        local kind: ChangeKind = if f.status == "added" then "add" elseif f.status == "deleted" then "del" else "mix"
        out[f.path] = kind
        local parts = string.split(f.path, "/")
        local dir = ""
        for i = 1, #parts - 1 do
            dir = if dir == "" then parts[i] else `{dir}/{parts[i]}`
            local was = out[dir]
            out[dir] = if was == nil or was == kind then kind else "mix"
        end
    end
    return out
end

-- Without `mix`: a change tree marks only added and deleted files, every row being a change.
function kinds.marks(all: { [string]: ChangeKind }): { [string]: ChangeKind }
    local out: { [string]: ChangeKind } = {}
    for path, kind in all do
        if kind ~= "mix" then
            out[path] = kind
        end
    end
    return out
end

type Node = { name: string, path: string, file: boolean }

-- Depth first, a folder's subfolders before its files, each by name (`sortReviewPaths`).
function kinds.sorted(paths: { string }): { string }
    local nodes: { [string]: Node } = {}
    for _, p in paths do
        local parts = string.split(p, "/")
        local path = ""
        for i, name in parts do
            path = if path == "" then name else `{path}/{name}`
            if nodes[path] == nil then
                nodes[path] = { name = name, path = path, file = i == #parts }
            end
        end
    end
    local children: { [string]: { Node } } = {}
    for _, node in nodes do
        local parent = string.match(node.path, "^(.*)/[^/]*$") or ""
        local list = children[parent]
        if list == nil then
            list = {}
            children[parent] = list
        end
        table.insert(list, node)
    end
    for _, list in children do
        table.sort(list, function(a: Node, b: Node): boolean
            if a.file ~= b.file then
                return not a.file
            end
            return a.name < b.name
        end)
    end
    local out: { string } = {}
    local function walk(parent: string)
        for _, node in children[parent] or {} do
            if node.file then
                table.insert(out, node.path)
            else
                walk(node.path)
            end
        end
    end
    walk("")
    return out
end

-- The paths in `files`.
function kinds.paths(files: { DiffFile }): { string }
    local out: { string } = {}
    for _, f in files do
        table.insert(out, f.path)
    end
    return out
end

return kinds
