--!strict
-- Each Office file the viewers show, as Desktop's file view and its worker machine take one
-- (`artifact-view.tsx` `ArtifactViewer`, `worker-machine.ts`): its bytes are read, bytes no
-- engine can open show the binary placeholder with why (`problem`), the rest open on a thread of
-- their own, and an engine that fails shows the placeholder too. Only the files shown last stay
-- open, so their threads give their memory back.

local machine = {}

-- Files kept open: the one showing and the one before it, so switching back is instant.
machine.KEPT = 2

export type Kind = "docx" | "xlsx" | "pptx"

export type Status =
    { kind: "reading", attempt: number }
    | { kind: "open", document: OfficeDocument, size: number }
    -- `reason` is a message key: `problem.compound`, `problem.format`, or none for a failed engine.
    | { kind: "binary", size: number, reason: string? }

export type File = { kind: Kind, directory: string, status: Status }

export type State = {
    issued: number,
    files: { [string]: File },
    -- Paths most recently shown last.
    order: { string },
}

export type Event =
    { type: "shown", path: string, directory: string, kind: Kind }
    | { type: "read", path: string, attempt: number, file: OfficeFile }
    | { type: "disposed" }

export type Effect =
    { type: "open", path: string, directory: string, kind: Kind, attempt: number }
    | { type: "close", document: OfficeDocument }

function machine.new(): State
    return { issued = 0, files = {}, order = {} }
end

local function forget(state: State, path: string, effects: { Effect })
    local file = state.files[path]
    if file and file.status.kind == "open" then
        table.insert(effects, { type = "close", document = file.status.document })
    end
    state.files[path] = nil
    local at = table.find(state.order, path)
    if at then
        table.remove(state.order, at)
    end
end

-- File path is shown again: it becomes the most recent, which stays open longest.
function machine.touch(state: State, path: string)
    local at = table.find(state.order, path)
    if at and at ~= #state.order then
        table.remove(state.order, at)
        table.insert(state.order, path)
    end
end

function machine.step(state: State, event: Event): { Effect }
    local effects: { Effect } = {}
    if event.type == "shown" then
        local file = state.files[event.path]
        local at = table.find(state.order, event.path)
        if file and file.kind == event.kind and file.directory == event.directory then
            if at ~= #state.order then
                table.remove(state.order, at :: number)
                table.insert(state.order, event.path)
            end
            return effects
        end
        if file then
            forget(state, event.path, effects)
        end
        state.issued += 1
        state.files[event.path] = { kind = event.kind, directory = event.directory, status = { kind = "reading", attempt = state.issued } }
        table.insert(state.order, event.path)
        table.insert(effects, { type = "open", path = event.path, directory = event.directory, kind = event.kind, attempt = state.issued })
        while #state.order > machine.KEPT do
            forget(state, state.order[1], effects)
        end
    elseif event.type == "read" then
        local file = state.files[event.path]
        local found = event.file
        -- A file shown again or let go since: its answer settles nothing, and its document goes.
        if not file or file.status.kind ~= "reading" or file.status.attempt ~= event.attempt then
            if found.document then
                table.insert(effects, { type = "close", document = found.document })
            end
            return effects
        end
        if found.document then
            file.status = { kind = "open", document = found.document, size = found.size }
        elseif found.signature == "compound" then
            file.status = { kind = "binary", size = found.size, reason = "problem.compound" }
        elseif found.signature == "other" then
            file.status = { kind = "binary", size = found.size, reason = "problem.format" }
        else
            file.status = { kind = "binary", size = found.size }
        end
    elseif event.type == "disposed" then
        for path in state.files do
            forget(state, path, effects)
        end
    end
    return effects
end

return machine
