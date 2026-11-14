--!strict
-- What the Updater plugin draws: Preferences > Updates (`section.tsx`), Settings > What's new
-- (`beta-page.tsx`), What's new after an update on a local or beta build (`beta-dialog.tsx`), and
-- the release highlights after one on the stable channel (`whats-new.tsx`).

local ui = require("@opencode/ui")
local notes = require("./notes")
local whatsnew = require("./whatsnew")

local view = {}

-- `settings-row-title` and `-description` as the catalog rows set them.
local SUMMARY = { size = 13, weight = 530, lineHeight = 16, tracking = -0.04, color = "text-chrome" :: Color, wrap = true }
local META = { size = 13, weight = 440, lineHeight = 20, tracking = -0.04, color = "text-muted" :: Color }
-- The dialog's own scale (`beta-dialog.tsx` `Meta`).
local DIALOG_SUMMARY = { size = 13, weight = 500, lineHeight = 20, tracking = -0.04, color = "text-chrome" :: Color, wrap = true }
local DIALOG_META = { size = 12, weight = 400, lineHeight = 16, color = "text-faint" :: Color }

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

-- The author's initial in a small ring, and their name, then the pull request; `my` is the
-- page's `settings-row-description` -3.5px margin.
local function meta(note: notes.Note, text: { [string]: any }, my: number, v: View, open: (url: string) -> ()): UiNode?
    if not note.author and not note.pr then
        return nil
    end
    local parts: { UiNode } = {}
    if note.author then
        local initial = string.upper(string.sub(note.author.name, 1, 1))
        table.insert(
            parts,
            ui.row({ w = 14, h = 14, radius = 7, center = true, fill = "surface-2", border = "border-base", borderWidth = 0.5 }, {
                ui.text(initial, { size = 8, weight = 530, lineHeight = 8, color = "text-muted" }),
            })
        )
        table.insert(parts, ui.text(note.author.name, text :: any))
    end
    if note.pr then
        local pr = note.pr
        if note.author then
            table.insert(parts, ui.text("·", text :: any))
        end
        table.insert(
            parts,
            ui.row({ key = `pr-{pr.number}`, role = "link", label = `#{pr.number}`, radius = 4, onPress = v:Handler(function()
                open(pr.url)
            end) }, { ui.text(`#{pr.number}`, { size = text.size, weight = text.weight, lineHeight = text.lineHeight, color = text.color, tabular = true }) })
        )
    end
    return ui.row({ gap = 6, my = my, align = "center", wrap = true }, parts)
end

-- Settings > What's new: every change this build carries, New then Fixes, in catalog lists.
function view.page(i18n: I18n, v: View, embedded: notes.Embedded, open: (url: string) -> ()): UiNode
    local all = notes.all(embedded)
    local sections: { UiNode } = {}
    for _, group in { { key = "added", title = i18n:T("beta.added"), list = all.added }, { key = "fixed", title = i18n:T("beta.fixed"), list = all.fixed } } do
        if #group.list == 0 then
            continue
        end
        local rows: { UiNode } = {}
        for _, note in group.list do
            table.insert(
                rows,
                ui.col({ key = `note-{note.sha}`, w = "full", py = 16, gap = 8 }, {
                    ui.text(note.summary, SUMMARY),
                    meta(note, META, -3.5, v, open),
                })
            )
        end
        table.insert(sections, ui.settingsList({ key = `whats-new-{group.key}`, title = group.title, catalog = true }, rows))
    end
    return ui.col({ key = "whats-new-body", w = "full", gap = 32 }, sections)
end

-- What's new after an update on a local or beta build: the listed features and fixes, and
-- View all when some are not listed.
function view.betaDialog(i18n: I18n, v: View, input: { notes: notes.Notes, previous: string, open: (url: string) -> (), viewAll: () -> (), done: () -> () }): UiNode
    local list: { UiNode } = {}
    for _, group in { { title = i18n:T("beta.added"), items = input.notes.added }, { title = i18n:T("beta.fixed"), items = input.notes.fixed } } do
        if #group.items == 0 then
            continue
        end
        local items: { UiNode } = {}
        for _, note in group.items do
            table.insert(items, ui.col({ w = "full", gap = 4 }, { ui.text(note.summary, DIALOG_SUMMARY), meta(note, DIALOG_META, 0, v, input.open) }))
        end
        table.insert(list, ui.col({ w = "full", gap = 12 }, { ui.text(group.title, { size = 12, weight = 530, lineHeight = 16, color = "text-muted" }), ui.col({ w = "full", gap = 16 }, items) }))
    end
    local footer: { UiNode } = {}
    if input.notes.more then
        table.insert(footer, ui.button({ key = "beta-view-all", text = i18n:T("beta.viewAll"), variant = "ghost", ml = -10, onPress = v:Handler(input.viewAll) }))
    end
    table.insert(footer, ui.spacer())
    table.insert(footer, ui.button({ key = "beta-done", text = i18n:T("beta.done"), variant = "contrast", onPress = v:Handler(input.done) }))
    return ui.col({ key = "beta-dialog", w = "full" }, {
        ui.col({ w = "full", pad = 16, gap = 4 }, {
            ui.text(i18n:T("beta.title"), { size = 15, weight = 530, lineHeight = 20, tracking = -0.13, color = "text-chrome" }),
            ui.text(i18n:T("beta.since", { count = input.notes.total, build = notes.build(input.previous) }), { size = 13, weight = 440, lineHeight = 20, tracking = -0.04, color = "text-muted" }),
        }),
        ui.col({ key = "beta-dialog-list", w = "full", maxH = 440, scrollY = true, px = 16, pb = 8, gap = 24 }, list),
        ui.row({ w = "full", pad = 16, gap = 8, align = "center" }, footer),
    })
end

-- The release highlights after an update on the stable channel: one at a time, Next until the
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
