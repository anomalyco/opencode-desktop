--!strict
-- The details popover's explicit machines: a footer action that reads the server's configuration
-- (`config-action-machine.ts`) and a location's MCP servers, their toggles and sign-ins in flight
-- and their error dialog (`mcp-machine.ts`). Each takes a state and an event and returns the
-- next state and the effects to run; the plugin runs the effects.

local machine = {}

-- How long reading the configuration and opening or copying its file may take.
machine.CONFIG_TIMEOUT = 15
-- How long a copied path keeps its mark.
machine.COPIED_FOR = 2
-- How long a switch may take before it frees up again.
machine.TOGGLE_TIMEOUT = 15

export type ConfigState =
    { status: "idle", issued: number }
    | { status: "opening", attempt: number }
    -- A remote server's configuration path is on the clipboard.
    | { status: "copied", attempt: number }

export type ConfigEvent =
    { type: "activate" }
    | { type: "opened", attempt: number, close: boolean }
    | { type: "copied", attempt: number }
    | { type: "failed", attempt: number, error: string }
    | { type: "timed-out", attempt: number }
    | { type: "copied.expired", attempt: number }
    | { type: "hidden" }

export type ConfigEffect =
    { type: "run", attempt: number }
    | { type: "schedule-reset", attempt: number }
    | { type: "close-owner" }
    | { type: "toast-failed", error: string }
    | { type: "toast-timed-out" }

machine.configIdle = { status = "idle", issued = 0 } :: ConfigState

function machine.config(state: ConfigState, event: ConfigEvent): (ConfigState, { ConfigEffect })
    if event.type == "hidden" then
        if state.status ~= "copied" then
            return state, {}
        end
        return { status = "idle", issued = state.attempt }, {}
    end
    if event.type == "activate" then
        if state.status == "opening" then
            return state, {}
        end
        local attempt = (if state.status == "idle" then state.issued else state.attempt) + 1
        return { status = "opening", attempt = attempt }, { { type = "run", attempt = attempt } }
    end
    if event.type == "copied.expired" then
        if state.status ~= "copied" or state.attempt ~= event.attempt then
            return state, {}
        end
        return { status = "idle", issued = state.attempt }, {}
    end
    if state.status ~= "opening" or state.attempt ~= event.attempt then
        return state, {}
    end
    local idle: ConfigState = { status = "idle", issued = state.attempt }
    if event.type == "opened" then
        return idle, if event.close then { { type = "close-owner" } } else {}
    elseif event.type == "copied" then
        return { status = "copied", attempt = state.attempt }, { { type = "schedule-reset", attempt = state.attempt } }
    elseif event.type == "failed" then
        return idle, { { type = "toast-failed", error = event.error } }
    end
    return idle, { { type = "toast-timed-out" } }
end

-- How long one connect, disconnect or sign-in may run before it gives up with an error.
machine.MCP_TOGGLE_TIMEOUT = 60

-- An MCP server's status, as the server lists it.
export type McpStatus = { status: string, error: string? }

-- A sign-in that also accepts the redirect URL pasted from the browser.
export type McpSignIn = { integrationID: string, attemptID: string, url: string, lifetime: number }

export type McpErrorDialog =
    { status: "closed" }
    -- One server's failure. `error` is its latest failure, kept while a retry runs; `copied` is
    -- the error text the user last copied.
    | { status: "open", attempt: number, name: string, error: string, copied: string? }

-- `McpState`: the MCP list of one server and directory, its toggles in flight (attempt by
-- name) and its error dialog, shared by the popover and the drawer that list it. Toggles and
-- dialogs take attempt numbers from `issued`, so a result whose attempt the state no longer
-- holds is stale. Desktop drops the state once nothing lists it; here it lives with the plugin.
export type McpState = { issued: number, toggles: { [string]: number }, dialog: McpErrorDialog }

export type McpEvent =
    -- A row's switch or sign-in button, or the error dialog's Disable or Try again.
    { type: "toggle.requested", name: string, enabled: boolean, status: string? }
    -- The server started an OAuth sign-in; the toggle goes on to refresh the server's list.
    | { type: "toggle.authorizing", name: string, attempt: number, url: string, signIn: McpSignIn? }
    | { type: "toggle.succeeded", name: string, attempt: number }
    -- `server` is true when the refreshed status failed, false when a request failed.
    | { type: "toggle.failed", name: string, attempt: number, error: string, server: boolean }
    | { type: "toggle.timed-out", name: string, attempt: number }
    | { type: "error.opened", name: string, error: string }
    -- The dialog closed, by the user or by the machine.
    | { type: "error.dismissed", attempt: number }
    | { type: "error.copied", attempt: number, error: string }
    -- The dialog's server changed in the server's list. `loaded` is false while the list is not known.
    | { type: "server.synced", attempt: number, loaded: boolean, status: McpStatus? }

export type McpEffect =
    { type: "run-toggle", name: string, attempt: number, enabled: boolean, timeout: number }
    | { type: "open-external", url: string }
    | { type: "open-sign-in", name: string, signIn: McpSignIn }
    | { type: "open-dialog", attempt: number, name: string }
    | { type: "close-dialog", attempt: number }
    | { type: "toast-failed", message: string }
    | { type: "toast-timed-out", name: string }

function machine.mcpInitial(): McpState
    return { issued = 0, toggles = {}, dialog = { status = "closed" } }
end

local function closeDialog(dialog: McpErrorDialog): { McpEffect }
    if dialog.status == "open" then
        return { { type = "close-dialog", attempt = dialog.attempt } }
    end
    return {}
end

-- `mcpRowView`: `busy` while the row's own toggle runs, `pending` while it takes no new toggle.
function machine.mcpRow(state: McpState, name: string, status: string?): (boolean, boolean)
    local busy = state.toggles[name] ~= nil
    return busy, busy or status == "pending"
end

-- `mcpDialogView`: what an open error dialog shows, or nil once it closed.
function machine.mcpDialog(state: McpState, attempt: number, status: string?): { name: string, error: string, copied: boolean, busy: boolean }?
    local dialog = state.dialog
    if dialog.status ~= "open" or dialog.attempt ~= attempt then
        return nil
    end
    return {
        name = dialog.name,
        error = dialog.error,
        copied = dialog.copied == dialog.error,
        busy = state.toggles[dialog.name] ~= nil or status == "pending",
    }
end

-- `transitionMcp` (`mcp-machine.ts`).
function machine.mcp(state: McpState, event: McpEvent): (McpState, { McpEffect })
    if event.type == "toggle.requested" then
        -- One toggle per server at a time, from its row and its dialog alike.
        if state.toggles[event.name] ~= nil or event.status == "pending" then
            return state, {}
        end
        local attempt = state.issued + 1
        local toggles = table.clone(state.toggles)
        toggles[event.name] = attempt
        return { issued = attempt, toggles = toggles, dialog = state.dialog },
            {
                {
                    type = "run-toggle",
                    name = event.name,
                    attempt = attempt,
                    enabled = event.enabled,
                    timeout = machine.MCP_TOGGLE_TIMEOUT,
                },
            }
    end
    if
        event.type == "toggle.authorizing"
        or event.type == "toggle.succeeded"
        or event.type == "toggle.failed"
        or event.type == "toggle.timed-out"
    then
        if state.toggles[event.name] ~= event.attempt then
            return state, {}
        end
        local dialog = state.dialog
        local shown = dialog.status == "open" and dialog.name == event.name
        -- The browser takes over from the dialog; the toggle still refreshes the server's list.
        if event.type == "toggle.authorizing" then
            local effects = if shown then closeDialog(dialog) else {}
            table.insert(effects, { type = "open-external", url = event.url })
            if event.signIn then
                table.insert(effects, { type = "open-sign-in", name = event.name, signIn = event.signIn })
            end
            return { issued = state.issued, toggles = state.toggles, dialog = if shown then { status = "closed" } else dialog },
                effects
        end
        local toggles = table.clone(state.toggles)
        toggles[event.name] = nil
        if event.type == "toggle.succeeded" then
            if shown then
                return { issued = state.issued, toggles = toggles, dialog = { status = "closed" } }, closeDialog(dialog)
            end
            return { issued = state.issued, toggles = toggles, dialog = dialog }, {}
        end
        if event.type == "toggle.timed-out" then
            return { issued = state.issued, toggles = toggles, dialog = dialog },
                { { type = "toast-timed-out", name = event.name } }
        end
        -- The open dialog shows the server's new failure itself.
        if shown and event.server and dialog.status == "open" then
            local open = table.clone(dialog)
            open.error = event.error
            return { issued = state.issued, toggles = toggles, dialog = open }, {}
        end
        local message = if event.server then `{event.name}: {event.error}` else event.error
        return { issued = state.issued, toggles = toggles, dialog = dialog }, { { type = "toast-failed", message = message } }
    end
    if event.type == "error.opened" then
        local attempt = state.issued + 1
        local effects = closeDialog(state.dialog)
        table.insert(effects, { type = "open-dialog", attempt = attempt, name = event.name })
        return {
            issued = attempt,
            toggles = state.toggles,
            dialog = { status = "open", attempt = attempt, name = event.name, error = event.error, copied = nil },
        },
            effects
    end
    local dialog = state.dialog
    if dialog.status ~= "open" or dialog.attempt ~= event.attempt then
        return state, {}
    end
    local closed: McpState = { issued = state.issued, toggles = state.toggles, dialog = { status = "closed" } }
    if event.type == "error.dismissed" then
        return closed, {}
    end
    if event.type == "error.copied" then
        local open = table.clone(dialog)
        open.copied = event.error
        return { issued = state.issued, toggles = state.toggles, dialog = open }, {}
    end
    if not event.loaded then
        return state, {}
    end
    local status = event.status
    -- A reconnect, a disable or a removal, here or elsewhere, leaves nothing to show.
    if status == nil or status.status == "connected" or status.status == "disabled" then
        return closed, closeDialog(dialog)
    end
    -- A retry stays open while it connects, and a new failure replaces the error in place.
    if status.status ~= "failed" or status.error == dialog.error then
        return state, {}
    end
    local open = table.clone(dialog)
    open.error = status.error or ""
    return { issued = state.issued, toggles = state.toggles, dialog = open }, {}
end

return machine
