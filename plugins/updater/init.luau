--!strict
-- Desktop's built-in Updater plugin (`packages/gui-plugins/src/updater`, `renderer.tsx`): the
-- Update pill in the titlebar or the sidebar's footer while an update is ready or installing,
-- Preferences > Updates (Release notes, Check for updates), the hidden Check for Updates…
-- command, and the checking, up-to-date, Update Ready, Update Available and Request failed
-- toasts of a check the user asked for (`actions.tsx`), Settings > What's new in builds that
-- carry their notes, and What's New after an update. The host's updater does the work
-- (`Updater`); this window's own requests follow `machine.luau` (`window-machine.ts`).

local plugin = require("@opencode/plugin")
local machine = require("@self/machine")
local action = require("@self/action")
local whatsnew = require("@self/whatsnew")
local view = require("@self/view")

type ReleaseNotes = { enabled: boolean }
type Seen = { version: string? }

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Updater = ctx:GetService("Updater")
    local Store = ctx:GetService("Store")
    local Toast = ctx:GetService("Toast")
    local Dialog = ctx:GetService("Dialog")
    local Contracts = ctx:GetService("Contracts")
    local App = ctx:GetService("App")
    local Http = ctx:GetService("Http")
    local Images = ctx:GetService("Images")
    local Theme = ctx:GetService("Theme")
    local releaseNotes = Store:Value("releaseNotes", { enabled = true }) :: StoreValue<ReleaseNotes>
    local seen = Store:Value("seen", {} :: Seen) :: StoreValue<Seen>
    local changed = ctx:CreateSignal()

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
    local function showOffer(version: string, ready: boolean)
        if offer then
            offer:Dismiss()
        end
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

    -- A different process placed this version. Withdraw a stale offer and explain why there
    -- is nothing to install. A manual check can ask to see the explanation again.
    local installedVersion: string? = nil
    local function installed(version: string, manual: boolean)
        if offer then
            offer:Dismiss()
            offer = nil
        end
        if manual or version ~= installedVersion then
            Toast:Show({ title = I18n:T("section.title"), description = I18n:T("toast.installed.description", { version = version }) })
        end
        installedVersion = version
    end
    -- `actions.tsx` `check`: a loading toast while it runs, then what it found.
    local function check()
        local checking = Toast:Show({ title = I18n:T("action.checking"), variant = "loading", persistent = true })
        local state = Updater:CheckAsync()
        checking:Dismiss()
        if state.phase == "disabled" then
            Toast:Show({ title = I18n:T("action.unavailable"), description = I18n:T("check.unavailable") })
        elseif state.phase == "installed" then
            installed(state.version or "", true)
        elseif state.phase == "up-to-date" then
            Toast:Show({
                title = I18n:T("toast.latest.title"),
                description = I18n:T("toast.latest.description", { version = App.Version }),
                variant = "success",
            })
        elseif state.phase == "ready" or state.phase == "download-required" then
            -- Desktop confirms its one download-required case (Beta moving to Stable) in a
            -- dialog; here it is Linux's every update, so it toasts like a ready one.
            showOffer(state.version or "", state.phase == "ready")
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

    -- `renderer.tsx`'s TitlebarItem: shown while an update is ready, offered as a download or
    -- installing, until the user installs it; its title names what a click does.
    ctx:AddTitlebarItem({
        id = "update",
        show = function()
            local state = Updater:State()
            local phase = state and state.phase
            if phase ~= "ready" and phase ~= "download-required" and phase ~= "installing" then
                return nil
            end
            return {
                label = I18n:T("status.label"),
                title = I18n:T(action(state, windowState).label),
                busy = phase == "installing",
            }
        end,
        run = function()
            act("install")
        end,
    })

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
            return v:Memo("updates", `{current.label}:{current.description or ""}:{current.run or ""}:{enabled}:{changed:Version()}:{I18n:Locale().id}`, function()
                return view.section(I18n, v, {
                    releaseNotes = enabled,
                    label = current.label,
                    description = current.description,
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

    -- The app menu's Check for Updates… checks in the focused window (`main.ts`'s beta path).
    local focused = true
    ctx.FocusChanged:Connect(function(f: boolean)
        focused = f
    end)
    Updater.StateChanged:Connect(function(state: UpdaterState?)
        if state and state.phase == "installed" and state.version then
            installed(state.version, false)
        end
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

    local highlightIndex = 1
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
        previousVersion = previous.version,
        enabled = releaseNotes:Get().enabled ~= false,
    })
    local timer: thread? = nil
    local dispatchWhatsNew: (event: whatsnew.Event) -> ()

    local function runWhatsNew(effect: whatsnew.Effect)
        if effect.type == "mark-seen" then
            seen:Set({ version = effect.version })
        elseif effect.type == "abort-load" then
            if timer then
                task.cancel(timer)
                timer = nil
            end
        elseif effect.type == "open" then
            highlightIndex = 1
            Dialog:Open("release", { highlights = effect.content.highlights })
        elseif effect.type == "load" then
            timer = task.delay(effect.timeout, function()
                timer = nil
                dispatchWhatsNew({ type = "load.timed-out" })
            end)
            task.spawn(function()
                local res = Http:FetchAsync({ url = whatsnew.CHANGELOG_URL, headers = { Accept = "application/json" }, timeout = effect.timeout })
                if not res.ok then
                    dispatchWhatsNew({ type = "load.failed" })
                    return
                end
                dispatchWhatsNew({ type = "loaded", content = { highlights = whatsnew.highlights(res.json, effect.version, effect.previousVersion) } })
            end)
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
        dispatchWhatsNew({ type = "disposed" })
    end)
end)
