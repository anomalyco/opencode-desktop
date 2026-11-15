--!strict
-- Desktop's built-in Updater plugin (`packages/gui-plugins/src/updater`, `renderer.tsx`):
-- Preferences > Updates (Release notes, Check for updates), the hidden Check for Updates…
-- command, the checking, up-to-date, Update Ready, Update Available and Request failed toasts,
-- the Update Ready or Update Available toast when the host's own checks find an update, once per
-- version (`notice.luau`; it replaces Desktop's titlebar update pill), Settings > What's
-- new in builds that carry their notes, and What's New after an update. The host's updater does
-- the work (`Updater`); this window's own requests follow `machine.luau` (`window-machine.ts`).

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")
local machine = require("@self/machine")
local notice = require("@self/notice")
local action = require("@self/action")
local notes = require("@self/notes")
local whatsnew = require("@self/whatsnew")
local view = require("@self/view")

type ReleaseNotes = { enabled: boolean }
type Seen = { version: string?, commit: string? }
type UpdateNotice = { version: string? }

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Updater = ctx:GetService("Updater")
    local Store = ctx:GetService("Store")
    local Toast = ctx:GetService("Toast")
    local Layout = ctx:GetService("Layout")
    local System = ctx:GetService("System")
    local Dialog = ctx:GetService("Dialog")
    local Contracts = ctx:GetService("Contracts")
    local App = ctx:GetService("App")
    local Http = ctx:GetService("Http")
    local Images = ctx:GetService("Images")
    local Theme = ctx:GetService("Theme")
    local releaseNotes = Store:Value("releaseNotes", { enabled = true }) :: StoreValue<ReleaseNotes>
    local seen = Store:Value("seen", {} :: Seen) :: StoreValue<Seen>
    local updateNotice = Store:Value("updateNotice", {} :: UpdateNotice) :: StoreValue<UpdateNotice>
    local changed = ctx:CreateSignal()
    local embedded = notes.embedded(App.BetaNotes)

    local windowState = machine.initial
    local dispatchWindow: (event: machine.Event) -> ()

    local function requestFailed(message: string?)
        Toast:Show({ title = I18n:T("common.requestFailed"), description = message })
    end

    local function install()
        local err = Updater:InstallAsync()
        if err then
            requestFailed(err)
        end
    end

    -- Update Ready (Restart / Later) or, for a download this install can't put in place by itself
    -- (Linux), Update Available (Download OpenCode / Later). One at a time: a newer one replaces it.
    local offer: ToastHandle? = nil
    local function showOffer(version: string, kind: notice.Kind)
        if offer then
            offer:Dismiss()
        end
        local ready = kind == "ready"
        offer = Toast:Show({
            title = I18n:T(if ready then "dialog.ready.title" else "dialog.available.title"),
            description = I18n:T(if ready then "dialog.ready.message" else "dialog.available.message", { version = version }),
            persistent = true,
            actions = {
                { label = I18n:T(if ready then "dialog.restart" else "action.download"), variant = "primary", run = function()
                    task.spawn(install)
                end },
                { label = I18n:T("dialog.later"), variant = "secondary" },
            },
        })
    end

    local noticeState = notice.initial
    local function dispatchNotice(event: notice.Event)
        local next, effects = notice.transition(noticeState, event)
        noticeState = next
        for _, effect in effects do
            if effect.type == "show" then
                showOffer(effect.version, effect.kind)
            else
                updateNotice:Set({ version = effect.version })
            end
        end
    end

    -- `actions.tsx` `check`: a loading toast while it runs, then what it found.
    local function check()
        local checking = Toast:Show({ title = I18n:T("action.checking"), variant = "loading", persistent = true })
        local state = Updater:CheckAsync()
        checking:Dismiss()
        if state.phase == "up-to-date" then
            Toast:Show({
                title = I18n:T("toast.latest.title"),
                description = I18n:T("toast.latest.description", { version = App.Version }),
                variant = "success",
            })
        elseif state.phase == "ready" or state.phase == "download-required" then
            -- Desktop confirms its one download-required case (Beta moving to Stable) in a
            -- dialog; here it is Linux's every update, so it toasts like a ready one.
            local version = state.version or ""
            showOffer(version, if state.phase == "ready" then "ready" else "download-required")
            dispatchNotice({ type = "announced", version = version })
        elseif state.phase == "error" then
            requestFailed(state.error)
        end
    end

    local function runWindow(effect: machine.Effect)
        if effect.type == "toast-failed" then
            requestFailed(effect.message)
            return
        end
        local attempt = effect.attempt
        local work = if effect.action == "check" then check else install
        task.spawn(function()
            local ok, err = pcall(work)
            if not ok then
                requestFailed(tostring(err))
            end
            dispatchWindow({ type = "action.settled", attempt = attempt })
        end)
    end

    dispatchWindow = function(event)
        local next, effects = machine.transition(windowState, event)
        windowState = next
        changed:Fire()
        for _, effect in effects do
            runWindow(effect)
        end
    end

    -- No updater in this build answers that nothing can check or install.
    local function act(name: machine.Action)
        dispatchWindow({ type = "action.requested", action = name, available = Updater:State() ~= nil })
    end

    ctx:AddSettingsPage({
        id = "updates",
        page = "general",
        title = I18n:T("section.title"),
        entries = {
            { id = "settings-release-notes", title = I18n:T("releaseNotes.title") },
            { id = "settings-check-updates", title = I18n:T("check.title") },
        },
        render = function(v)
            local state = Updater:State()
            local current = action(state, windowState)
            local enabled = releaseNotes:Get().enabled ~= false
            return v:Memo("updates", `{current.label}:{current.run}:{enabled}:{changed:Version()}:{I18n:Locale().id}`, function()
                return view.section(I18n, v, {
                    releaseNotes = enabled,
                    label = current.label,
                    -- Disabled while the action runs, or with no updater in this build.
                    enabled = current.run ~= nil and state ~= nil,
                    toggle = function(on)
                        releaseNotes:Set({ enabled = on })
                    end,
                    run = function()
                        local run = action(Updater:State(), windowState).run
                        if run then
                            act(run)
                        end
                    end,
                })
            end)
        end,
    })

    ctx:AddCommand({
        id = "check",
        title = I18n:T("menu.check"),
        hidden = true,
        run = function()
            act("check")
        end,
    })

    -- The app menu's Check for Updates… checks in the focused window (`main.ts`'s beta path), and
    -- an update the host's own checks found is announced there.
    local focused = true
    ctx.FocusChanged:Connect(function(f: boolean)
        focused = f
        if f then
            dispatchNotice({ type = "focused", notified = updateNotice:Get().version })
        end
    end)
    Updater.StateChanged:Connect(function(state: UpdaterState?)
        dispatchNotice({ type = "updater", phase = state and state.phase, version = state and state.version, focused = focused, notified = updateNotice:Get().version })
    end)
    local menu = Contracts:Use("updater")
    local checks = if menu:State() == "active" then (menu:Get() :: any).checks else 0
    menu.Changed:Connect(function()
        if menu:State() ~= "active" then
            return
        end
        local now = (menu:Get() :: any).checks
        if now ~= checks then
            checks = now
            if focused then
                act("check")
            end
        end
    end)

    -- Settings > What's new, in builds that carry their notes.
    if embedded then
        local notesOf = embedded :: notes.Embedded
        ctx:AddSettingsPage({
            id = "whats-new",
            icon = "outline-list-changelog",
            about = true,
            title = I18n:T("beta.title"),
            description = I18n:T("beta.page.description"),
            entries = { { id = "whats-new", title = I18n:T("beta.title"), keywords = "changelog release notes beta" } },
            render = function(v)
                return v:Memo("whats-new", I18n:Locale().id, function()
                    return view.page(I18n, v, notesOf, function(url)
                        System:OpenExternal(url)
                    end)
                end)
            end,
            renderActions = function(v)
                return ui.button({
                    key = "settings-whats-new-history",
                    text = I18n:T("beta.github"),
                    variant = "ghost-muted",
                    onPress = v:Handler(function()
                        System:OpenExternal(notes.history(notesOf))
                    end),
                })
            end,
        })
    end

    local highlightIndex = 1
    ctx:AddDialog({
        id = "beta",
        size = "normal",
        render = function(v, props)
            local data = (props.data or {}) :: any
            return v:Memo("beta-dialog", I18n:Locale().id, function()
                return view.betaDialog(I18n, v, {
                    notes = data.notes,
                    previous = data.previous or "",
                    open = function(url)
                        System:OpenExternal(url)
                    end,
                    viewAll = function()
                        Dialog:Close()
                        Layout:OpenSettings("whats-new")
                    end,
                    done = function()
                        Dialog:Close()
                    end,
                })
            end)
        end,
    })
    ctx:AddDialog({
        id = "release",
        size = "large",
        render = function(v, props)
            local data = (props.data or {}) :: any
            -- The shown feature's picture landing or failing, or its video failing, rebuilds the dialog.
            local feature = (data.highlights or {})[highlightIndex]
            local media = feature and feature.media
            local picture = if media and media.type == "image" then Images:State(Images:Load(media.src)).status
                elseif media and media.type == "video" and Images:VideoFailed(media.src) then "failed"
                else ""
            local dark = Theme:IsDark()
            return v:Memo("release-dialog", `{highlightIndex}:{picture}:{dark}:{I18n:Locale().id}`, function()
                return view.highlights(I18n, v, {
                    highlights = data.highlights or {},
                    index = highlightIndex,
                    dark = dark,
                    go = function(i)
                        highlightIndex = i
                        changed:Fire()
                    end,
                    close = function()
                        Dialog:Close()
                    end,
                    disable = function()
                        releaseNotes:Set({ enabled = false })
                        Dialog:Close()
                    end,
                })
            end)
        end,
    })

    -- What's New after an update (`whats-new-machine.ts`).
    local previous = seen:Get()
    local whatsState, startEffects = whatsnew.init({
        version = App.Version,
        commit = App.Commit,
        channel = if App.Channel == "beta" then "beta" else "stable",
        previousVersion = previous.version,
        previousCommit = previous.commit,
        enabled = releaseNotes:Get().enabled ~= false,
    })
    local timer: thread? = nil
    local dispatchWhatsNew: (event: whatsnew.Event) -> ()

    local function runWhatsNew(effect: whatsnew.Effect)
        if effect.type == "mark-seen" then
            local next: Seen = { version = effect.version, commit = effect.commit or seen:Get().commit }
            seen:Set(next)
        elseif effect.type == "abort-load" then
            if timer then
                task.cancel(timer)
                timer = nil
            end
        elseif effect.type == "open" then
            local content = effect.content
            if content.channel == "beta" then
                Dialog:Open("beta", { notes = content.notes, previous = effect.previousVersion })
            else
                highlightIndex = 1
                Dialog:Open("release", { highlights = content.highlights })
            end
        elseif effect.type == "load" then
            timer = task.delay(effect.timeout, function()
                timer = nil
                dispatchWhatsNew({ type = "load.timed-out" })
            end)
            if effect.channel == "beta" then
                local since = embedded and notes.since(embedded, effect.previousCommit)
                task.defer(dispatchWhatsNew, { type = "loaded", content = if since then { channel = "beta", notes = since } else nil })
            else
                task.spawn(function()
                    local res = Http:FetchAsync({ url = whatsnew.CHANGELOG_URL, headers = { Accept = "application/json" }, timeout = effect.timeout })
                    if not res.ok then
                        dispatchWhatsNew({ type = "load.failed" })
                        return
                    end
                    dispatchWhatsNew({ type = "loaded", content = { channel = "stable", highlights = whatsnew.highlights(res.json, effect.version, effect.previousVersion) } })
                end)
            end
        end
    end

    dispatchWhatsNew = function(event)
        local next, effects = whatsnew.transition(whatsState, event)
        whatsState = next
        for _, effect in effects do
            runWhatsNew(effect)
        end
    end

    for _, effect in startEffects do
        runWhatsNew(effect)
    end
    -- Turning release notes off while the notes load means they never open.
    releaseNotes.Changed:Connect(function()
        if whatsState.status == "loading" and releaseNotes:Get().enabled == false then
            dispatchWhatsNew({ type = "disabled" })
        end
    end)

    ctx.Disposing:Connect(function()
        dispatchWindow({ type = "disposed" })
        dispatchNotice({ type = "disposed" })
        dispatchWhatsNew({ type = "disposed" })
    end)
end)
