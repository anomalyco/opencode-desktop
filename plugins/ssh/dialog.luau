--!strict
-- Add SSH server and Connect to <host> (Desktop's `dialog.tsx`, `dialog-machine.ts`): the target
-- and an optional name, then each sign-in prompt the connect raises (a password, a key's
-- passphrase, a host's fingerprint to trust), until the server is up. The dialog follows the app
-- half's state for its server; closing it while its attempt runs cancels it.

local ui = require("@opencode/ui")
local command = require("./command")

local dialog = {}

-- `idle` shows the form; `active` follows the attempt the dialog started (or, in prompt mode,
-- the one it opened for), `prompted` once it showed a prompt; `completed` closes it on a ready
-- server; `closed` is a dialog that went away.
export type State = {
    status: "idle" | "active" | "completed" | "closed",
    id: string,
    mode: "add" | "connect" | "prompt",
    target: string,
    name: string,
    prompted: boolean,
    challenge: { prompt: { id: string, text: string, confirm: boolean }, response: string }?,
}

-- What this window's controller and the app half's state show for the dialog's server.
export type Snapshot = { item: any, submitting: boolean, error: boolean, answeredPrompt: string?, activeAttempt: boolean }

function dialog.create(id: string, config: any?, promptOnly: boolean?): State
    local target = if config then config.target else ""
    local name = if config then config.name else ""
    if promptOnly then
        return { status = "active", id = id, mode = "prompt", target = target, name = name, prompted = true }
    end
    return { status = "idle", id = id, mode = if config then "connect" else "add", target = target, name = name, prompted = false }
end

local function stageOf(snapshot: Snapshot): string?
    return if snapshot.item then snapshot.item.stage else nil
end

-- A call is in flight, or the dialog's own attempt connects.
local function isBusy(s: State, snapshot: Snapshot): boolean
    return snapshot.submitting or (s.status == "active" and snapshot.activeAttempt and not snapshot.error and (stageOf(snapshot) or "connecting") == "connecting")
end

local function isWaiting(s: State, snapshot: Snapshot): boolean
    return isBusy(s, snapshot) or (s.status == "active" and s.challenge ~= nil and not snapshot.error and snapshot.answeredPrompt == s.challenge.prompt.id)
end

local function with(s: State, changes: { [string]: any }): State
    local copy = table.clone(s) :: any
    for k, v in changes do
        copy[k] = v
    end
    return copy
end

-- One step (`transitionSshDialog`). Effects: `connect`, `respond`, `cancel`, `complete`.
function dialog.transition(s: State, e: { [string]: any }): (State, { { [string]: any } })
    if s.status == "completed" or s.status == "closed" then
        return s, {}
    end
    if e.type == "disposed" then
        return with(s, { status = "closed" }), if s.status == "active" then { { type = "cancel" } } else {}
    end
    if e.type == "target.changed" then
        return if s.target == e.value then s else with(s, { target = e.value }), {}
    end
    if e.type == "name.changed" then
        return if s.name == e.value then s else with(s, { name = e.value }), {}
    end
    if e.type == "submitted" then
        if isBusy(s, e.snapshot) or #(string.match(s.target, "^%s*(.-)%s*$") or "") == 0 or (s.status == "active" and s.challenge) then
            return s, {}
        end
        -- Each start forgets the prompts shown before it.
        local nextState: State = { status = "active", id = s.id, mode = s.mode, target = s.target, name = s.name, prompted = false }
        return nextState, { { type = "connect", config = { id = s.id, target = s.target, name = s.name }, replace = stageOf(e.snapshot) == "incompatible" } }
    end
    if s.status == "idle" then
        return s, {}
    end
    local challenge = s.challenge
    if e.type == "response.changed" then
        if not challenge or challenge.prompt.id ~= e.prompt or challenge.response == e.value then
            return s, {}
        end
        return with(s, { challenge = { prompt = challenge.prompt, response = e.value } }), {}
    end
    if e.type == "responded" then
        local item = e.snapshot.item
        if not challenge or not item or not item.prompt or item.prompt.id ~= challenge.prompt.id or isWaiting(s, e.snapshot) or (not challenge.prompt.confirm and #challenge.response == 0) then
            return s, {}
        end
        return s, { { type = "respond", prompt = challenge.prompt.id, value = if challenge.prompt.confirm then "yes" else challenge.response } }
    end
    -- synced
    local snapshot: Snapshot = e.snapshot
    if not snapshot.submitting and stageOf(snapshot) == "ready" then
        return with(s, { status = "completed" }), { { type = "complete" } }
    end
    local prompt = if snapshot.item then snapshot.item.prompt else nil
    if prompt then
        if challenge and challenge.prompt.id == prompt.id then
            return s, {}
        end
        return with(s, { prompted = true, challenge = { prompt = prompt, response = "" } }), {}
    end
    -- The prompt this window answered stays, disabled, while the app half checks the answer;
    -- any other goes once it clears it.
    if not challenge or (snapshot.answeredPrompt == challenge.prompt.id and not snapshot.error and (snapshot.submitting or stageOf(snapshot) == "connecting")) then
        return s, {}
    end
    local cleared = table.clone(s)
    cleared.challenge = nil
    return cleared, {}
end

-- What the dialog shows (`viewSshDialog`).
function dialog.view(s: State, snapshot: Snapshot)
    local shown = s.status == "active" or s.status == "completed"
    local prompted = shown and s.prompted
    local challenge = if shown then s.challenge else nil
    local incompatible = stageOf(snapshot) == "incompatible"
    local settled = s.status == "completed" or s.status == "closed"
    local busy = isBusy(s, snapshot)
    local waiting = isWaiting(s, snapshot) or settled
    -- The attempt ended without a server or an error, as when the app half goes away as it starts.
    local lost = s.status == "active" and not snapshot.submitting and not snapshot.activeAttempt and not snapshot.item and not snapshot.error
    local err: string? = nil
    if snapshot.error or lost then
        err = "common.requestFailed"
    elseif snapshot.item and snapshot.item.error then
        err = `error.{snapshot.item.error}`
    end
    local primary
    if challenge then
        primary = {
            action = "respond",
            disabled = waiting or (not challenge.prompt.confirm and #challenge.response == 0),
            label = if waiting then "stage.connecting" elseif challenge.prompt.confirm then "trust" else "continue",
        }
    else
        primary = {
            action = "submit",
            disabled = busy or #(string.match(s.target, "^%s*(.-)%s*$") or "") == 0 or settled,
            label = if busy then "stage.connecting" elseif incompatible then "update" elseif s.mode == "add" then "form.add" else "connect",
        }
    end
    return {
        title = if prompted or s.mode ~= "add" then "connectTo" else "add",
        showForm = s.mode ~= "prompt" and not incompatible and (not prompted or (err ~= nil and challenge == nil)),
        inputsDisabled = busy or challenge ~= nil or settled,
        incompatible = incompatible,
        prompt = if challenge then challenge.prompt else nil,
        waiting = waiting,
        error = if incompatible then nil else err,
        primary = primary,
    }
end

-- The dialog's body inside the host's frame.
function dialog.render(env: any, v: View): UiNode?
    local c = env.dialog
    if not c.state then
        return nil
    end
    local s: State = c.state
    local snapshot = c.snapshot()
    local view = dialog.view(s, snapshot)
    local I18n = env.I18n
    local host = command.name({ target = if view.title == "connectTo" and #env.target:Text() == 0 and snapshot.item then snapshot.item.config.target else env.target:Text(), name = env.name:Text() })
    local title = if view.title == "connectTo" then I18n:T("connectTo", { host = host }) else I18n:T("add")
    local label = function(text: string): UiNode
        return ui.text(text, { size = 13, weight = "medium", lineHeight = 16, color = "text-chrome" })
    end
    local body: { UiNode } = {}
    if view.showForm then
        table.insert(body, ui.col({ w = "full", gap = 8 }, {
            label(I18n:T("target")),
            ui.field({ key = "ssh-target", editor = env.target, placeholder = I18n:T("placeholder"), size = "large", invalid = view.error ~= nil, disabled = view.inputsDisabled, w = "full", dir = "ltr" }),
        }))
        table.insert(body, ui.col({ w = "full", gap = 8 }, {
            label(I18n:T("form.name")),
            ui.field({ key = "ssh-name", editor = env.name, placeholder = I18n:T("form.namePlaceholder"), size = "large", disabled = view.inputsDisabled, w = "full", dir = "auto" }),
        }))
    end
    if view.incompatible then
        table.insert(body, ui.col({ w = "full", gap = 8, role = "status" }, {
            ui.text(I18n:T("stage.incompatible"), { size = 14, weight = "medium", lineHeight = 20, color = "text-chrome" }),
            ui.text(I18n:T("error.version"), { size = 13, weight = "regular", lineHeight = 20, color = "text-muted", wrap = true }),
        }))
    end
    if view.prompt then
        local block: { UiNode } = {
            ui.col({ key = "ssh-prompt", w = "full", maxH = 200, scrollY = true }, {
                ui.text(view.prompt.text, { mono = true, size = 12, weight = "regular", lineHeight = 16, wrap = true, selectable = true, color = "text-chrome", dir = "auto" }),
            }),
        }
        if not view.prompt.confirm then
            table.insert(block, ui.field({ key = "ssh-response", editor = env.response, size = "large", secure = true, disabled = view.waiting, w = "full" }))
        end
        table.insert(body, ui.col({ w = "full", gap = 8 }, block))
    end
    if view.error then
        table.insert(body, ui.text(I18n:T(view.error), { key = "ssh-error", size = 11, weight = "regular", lineHeight = 16, wrap = true, color = "status-danger", role = "status" }))
    end
    return ui.col({ key = "ssh-dialog", w = "full" }, {
        ui.row({ w = "full", pad = 16, align = "center" }, { ui.text(title, { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text-chrome" }) }),
        ui.box({ w = "full", h = 0.5, fill = "border-strong" }),
        ui.col({ w = "full", px = 16, pt = 16, pb = 8, gap = 24 }, body),
        ui.row({ w = "full", pad = 16, gap = 8 }, {
            ui.spacer(),
            ui.button({ key = "ssh-cancel", text = I18n:T("common.cancel"), variant = "neutral", onPress = v:Handler(function()
                env.close()
            end) }),
            ui.button({ key = "ssh-primary", text = I18n:T(view.primary.label), variant = "contrast", disabled = view.primary.disabled, onPress = v:Handler(function()
                if view.primary.action == "respond" then
                    c.respond()
                else
                    c.submit()
                end
            end) }),
        }),
    })
end

return dialog
