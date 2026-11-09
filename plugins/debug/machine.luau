--!strict
-- Capture heap snapshots (`heap-machine.ts`): one capture at a time, each with its attempt; the
-- command does nothing while one runs. `step` returns the effects to run.

local machine = {}

export type Heap = { status: "idle", attempt: number } | { status: "capturing", attempt: number }

export type HeapEvent = { type: "requested" } | { type: "settled", attempt: number, path: string?, error: string? }

export type HeapEffect =
    { type: "capture", attempt: number }
    | { type: "toast-loading" }
    | { type: "toast-saved", path: string }
    | { type: "toast-partial", path: string }
    | { type: "toast-failed" }

machine.idle = { status = "idle", attempt = 0 } :: Heap

function machine.heap(state: Heap, event: HeapEvent): (Heap, { HeapEffect })
    if event.type == "requested" then
        if state.status == "capturing" then
            return state, {}
        end
        local attempt = state.attempt + 1
        return { status = "capturing", attempt = attempt }, {
            { type = "toast-loading" },
            { type = "capture", attempt = attempt },
        }
    end
    if state.status ~= "capturing" or state.attempt ~= event.attempt then
        return state, {}
    end
    local idle: Heap = { status = "idle", attempt = state.attempt }
    if event.path == nil then
        return idle, { { type = "toast-failed" } }
    end
    if event.error then
        return idle, { { type = "toast-partial", path = event.path } }
    end
    return idle, { { type = "toast-saved", path = event.path } }
end

return machine
