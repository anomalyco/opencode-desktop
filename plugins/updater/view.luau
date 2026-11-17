--!strict
-- What the Updater plugin draws: Preferences > Updates (`section.tsx`) and the release
-- highlights after an update (`whats-new.tsx`).

local ui = require("@opencode/ui")
local whatsnew = require("./whatsnew")

local view = {}

-- Preferences > Updates: the Release notes switch and Check for updates with the one action
-- the state offers.
function view.section(i18n: I18n, v: View, input: { releaseNotes: boolean, label: string, enabled: boolean, toggle: (on: boolean) -> (), run: () -> () }): UiNode
    return ui.settingsList({ key = "settings-updates", title = i18n:T("section.title") }, {
        ui.settingsRow(
            { id = "settings-release-notes", title = i18n:T("releaseNotes.title"), description = i18n:T("releaseNotes.description") },
            ui.switch({
                key = "release-notes-switch",
                label = i18n:T("releaseNotes.title"),
                checked = input.releaseNotes,
                onChange = v:Handler(function(event)
                    input.toggle((event :: any).checked == true)
                end),
            })
        ),
        ui.settingsRow(
            { id = "settings-check-updates", title = i18n:T("check.title"), description = i18n:T("check.description") },
            ui.button({
                key = "check-updates-button",
                text = i18n:T(input.label),
                disabled = not input.enabled,
                onPress = v:Handler(input.run),
            })
        ),
    })
end

-- The release highlights after an update: one at a time, Next until the
-- last, then Get started; Don't show these in the future turns release notes off.
function view.highlights(i18n: I18n, v: View, input: { highlights: { whatsnew.Highlight }, index: number, dark: boolean, go: (index: number) -> (), close: () -> (), disable: () -> () }): UiNode
    local total = #input.highlights
    local index = math.clamp(input.index, 1, math.max(1, total))
    local feature = input.highlights[index]
    local last = index >= total
    local dots: { UiNode } = {}
    -- `bg-icon-strong-base` and `bg-icon-weak-base`, Desktop's v1 icon tokens, which no semantic
    -- token names: as its default theme resolves them in each scheme.
    local strong = if input.dark then ui.hex("#ede8e4") else ui.hex("#1d1917")
    local weak = if input.dark then ui.hex("#343434") else ui.hex("#c7c7c7")
    if total > 1 then
        for i = 1, total do
            local current = i == index
            table.insert(
                dots,
                ui.row({ key = `dot-{i}`, role = "button", label = `{i}`, w = if current then 32 else 12, h = 24, align = "center", onPress = v:Handler(function()
                    input.go(i)
                end) }, { ui.box({ w = "full", h = 2, radius = 1, fill = if current then strong else weak }) })
            )
        end
    end
    -- The feature's picture or video, edge to edge on the right (`whats-new.tsx` media): on the
    -- faint surface tint, covering its box, its outer corners rounded 12.
    local media: { UiNode } = {}
    if feature and feature.media then
        local m = feature.media
        local label = m.alt or feature.title or i18n:T("releaseNotes.media.alt")
        -- `bg-surface-base`: Desktop's `--surface-base`, 3.1 % black, or white in dark
        -- (`theme.css:99`, `:370`), which no semantic token names.
        local tint = if input.dark then ui.hex("#ffffff08") else ui.hex("#00000008")
        media = {
            ui.box({ key = `release-media-{index}`, grow = 1, minW = 0, h = "full", fill = tint, clip = true, corners = { 0, 12, 12, 0 } }, {
                if m.type == "image"
                    then ui.image({ key = "release-media-image", src = m.src, w = "full", h = "full", fit = "cover", label = label })
                    else ui.video({ key = "release-media-video", src = m.src, w = "full", h = "full", fit = "cover", label = label }),
            }),
        }
    end
    -- `text-16-medium` and `text-14-regular`: 180 % line height.
    local text = ui.col({ key = "release-text", grow = 1, minW = 0, h = "full", pad = 32 }, {
        ui.col({ gap = 8, pt = 88 }, {
            ui.text(if feature then feature.title else "", { size = 16, weight = 530, lineHeight = 28.8, color = "text-strong", wrap = true }),
            ui.text(if feature then feature.description else "", { size = 14, weight = 440, lineHeight = 25.2, color = "text-base", wrap = true }),
        }),
        ui.spacer(),
        ui.col({ gap = 48 }, {
            ui.col({ gap = 12, align = "start" }, {
                if last
                    then ui.button({ key = "release-get-started", text = i18n:T("releaseNotes.action.getStarted"), variant = "contrast", size = "large", onPress = v:Handler(input.close) })
                    else ui.button({ key = "release-next", text = i18n:T("releaseNotes.action.next"), size = "large", onPress = v:Handler(function()
                        input.go(index + 1)
                    end) }),
                ui.button({ key = "release-hide", text = i18n:T("releaseNotes.action.hideFuture"), variant = "ghost", size = "small", onPress = v:Handler(input.disable) }),
            }),
            if #dots > 0 then ui.row({ gap = 6, my = -10, align = "center" }, dots) else nil,
        }),
    })
    table.insert(media, 1, text)
    -- Desktop's dialog is `size="large"` (`whats-new.tsx`): the text and the picture share its
    -- width (`flex-1 p-8` beside `flex-1`) and fill its height.
    return ui.row({ key = "release-notes", w = "full", h = "full", clip = true }, media)
end

return view
