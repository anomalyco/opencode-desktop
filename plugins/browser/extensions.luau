--!strict
-- Settings > Browser > Extensions section, "Add an extension" dialog, and InstallReview
-- confirmation step matching upstream `packages/gui-plugins/src/browser/{settings.tsx,
-- settings-view.ts,choices.tsx}` (`b9a60d9d07`) and `proof/reference/e51247f626/ws9/`.

local ui = require("@opencode/ui")

local extensions = {}

export type Compatibility = {
    level: "supported" | "partial" | "unsupported",
    cause: string?,
    missing: { string }?,
}

export type ProfileExtension = {
    id: string,
    name: string,
    version: string,
    enabled: boolean,
    source: string,
    icon: string?,
    error: string?,
    changed: boolean?,
    compatibility: Compatibility,
    access: { string }?,
}

export type BrowserProfileEntry = {
    id: string,
    name: string,
}

export type BrowserSource = {
    id: string,
    name: string,
    profiles: { BrowserProfileEntry },
}

export type ProfileChoice = {
    sourceId: string,
    sourceName: string,
    profileId: string,
    profileName: string,
    multiProfile: boolean,
}

export type BrowserCandidate = {
    id: string,
    name: string,
    version: string,
    icon: string?,
    installed: boolean,
    compatibility: Compatibility,
    access: { string }?,
}

export type ReviewedGrants = {
    access: { string }?,
    hosts: { string }?,
    permissions: { string }?,
    externally_connectable: { string }?,
    web_accessible_resources: { string }?,
}

export type InstallReview = {
    unsupported: boolean,
    token: string,
    id: string,
    source: string,
    store: string?,
    name: string,
    version: string,
    icon: string?,
    replaces: string?,
    compatibility: Compatibility,
    grants: ReviewedGrants,
    sites: { string }?,
    added: ReviewedGrants?,
}

export type DialogMessage = {
    text: string,
    error: boolean,
    compatibility: Compatibility?,
}

export type DialogState = {
    busy: string?,
    installingId: string?,
    choices: { ProfileChoice },
    selectedKey: string?,
    candidates: { BrowserCandidate },
    candidatesLoaded: boolean,
    devOpen: boolean,
    review: InstallReview?,
    showAllSites: boolean,
    approving: boolean,
    message: DialogMessage?,
}

export type SectionEnv = {
    I18n: I18nService,
    available: boolean,
    busy: boolean,
    list: { ProfileExtension },
    onOpenAdd: () -> (),
    onToggle: (id: string, enabled: boolean) -> (),
    onRemove: (id: string) -> (),
}

export type DialogEnv = {
    I18n: I18nService,
    linkEditor: Editor,
    state: DialogState,
    onAddLink: () -> (),
    onSelectProfile: (key: string) -> (),
    onOpenProfileMenu: () -> (),
    onAddFromBrowser: (sourceId: string, profileId: string, extId: string) -> (),
    onToggleDev: () -> (),
    onPickFolder: () -> (),
    onToggleSites: () -> (),
    onApprove: (token: string) -> (),
    onCancelReview: () -> (),
    onClose: () -> (),
}

local TITLE: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", truncate = true } :: UiTextProps)
local BODY: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 18, color = "text-muted", wrap = true } :: UiTextProps)
local BODY_SINGLE: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 18, color = "text-muted", noWrap = true } :: UiTextProps)
local NOTE: UiTextProps = table.freeze({ size = 12, weight = "regular", lineHeight = 16, color = "text-faint", wrap = true } :: UiTextProps)
local SUB_MUTED: UiTextProps = table.freeze({ size = 12, weight = "regular", lineHeight = 16, color = "text-muted", noWrap = true } :: UiTextProps)
local SUB_WARN: UiTextProps = table.freeze({ size = 12, weight = "regular", lineHeight = 16, color = "status-warning", noWrap = true } :: UiTextProps)
local WARN_LINE: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 18, color = "status-warning", noWrap = true } :: UiTextProps)
local DANGER_LINE: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 18, color = "status-danger", wrap = true } :: UiTextProps)
local DIALOG_TITLE: UiTextProps = table.freeze({ size = 15, weight = "medium", lineHeight = 20, color = "text-chrome", wrap = true } :: UiTextProps)
local SECTION_LABEL: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", noWrap = true } :: UiTextProps)
local WARN_LABEL: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 18, color = "status-warning", wrap = true } :: UiTextProps)
local STRONG_LINE: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 18, color = "text-chrome", wrap = true } :: UiTextProps)

function extensions.flattenChoices(sources: { BrowserSource }): { ProfileChoice }
    local out: { ProfileChoice } = {}
    for _, src in sources do
        local multi = #src.profiles > 1
        for _, prof in src.profiles do
            table.insert(out, {
                sourceId = src.id,
                sourceName = src.name,
                profileId = prof.id,
                profileName = prof.name,
                multiProfile = multi,
            })
        end
    end
    return out
end

function extensions.choiceKey(choice: ProfileChoice): string
    return `{choice.sourceId}\n{choice.profileId}`
end

function extensions.choiceLabel(choice: ProfileChoice): string
    if choice.multiProfile then
        return `{choice.sourceName} · {choice.profileName}`
    end
    return choice.sourceName
end

function extensions.findChoice(choices: { ProfileChoice }, key: string?): ProfileChoice?
    if key ~= nil then
        for _, c in choices do
            if extensions.choiceKey(c) == key then
                return c
            end
        end
    end
    return choices[1]
end

function extensions.isHardBlocked(compat: Compatibility?): boolean
    if compat == nil then
        return false
    end
    return compat.cause == "manifest_v2" or compat.cause == "disallowed_permission"
end

function extensions.failureMessage(i18n: I18nService, reason: string?): string
    if reason == "manifest" then
        return i18n:T("extensions.failed.manifest")
    elseif reason == "download" then
        return i18n:T("extensions.failed.download")
    elseif reason == "collision" then
        return i18n:T("extensions.failed.collision")
    elseif reason == "expired" then
        return i18n:T("extensions.failed.expired")
    elseif reason == "downgrade" then
        return i18n:T("extensions.failed.downgrade")
    elseif reason == "load" then
        return i18n:T("extensions.failed.load")
    end
    return i18n:T("settings.failed.install")
end

function extensions.installedMessage(i18n: I18nService, ext: ProfileExtension): DialogMessage
    local level = if ext.compatibility then ext.compatibility.level else "supported"
    if level == "unsupported" then
        return {
            text = i18n:T("extensions.added.unsupported", { name = ext.name }),
            error = false,
            compatibility = ext.compatibility,
        }
    elseif level == "partial" then
        return {
            text = i18n:T("extensions.added.partial", { name = ext.name }),
            error = false,
            compatibility = ext.compatibility,
        }
    end
    return {
        text = i18n:T("extensions.added", { name = ext.name }),
        error = false,
        compatibility = nil,
    }
end

local function compatTooltip(i18n: I18nService, compat: Compatibility?): string?
    if compat == nil or compat.level == "supported" then
        return nil
    end
    local lines: { string } = {}
    if compat.cause ~= nil and compat.cause ~= "" then
        local causeKey = `extensions.compat.cause.{compat.cause}`
        local causeText = i18n:T(causeKey)
        if causeText ~= causeKey then
            table.insert(lines, causeText)
        end
    end
    local missing: { string } = compat.missing or {}
    if #missing > 0 then
        table.insert(lines, i18n:T("extensions.compat.missing"))
        for _, feat in missing do
            local fk = `extensions.compat.feature.{feat}`
            local label = i18n:T(fk)
            table.insert(lines, `  • {if label ~= fk then label else feat}`)
        end
    end
    if #lines == 0 then
        return nil
    end
    return table.concat(lines, "\n")
end

local function glyph(iconUrl: string?, fallback: IconName): ui.Node
    local fallbackNode = ui.box({ w = 20, h = 20, center = true }, {
        ui.icon(fallback, { size = 16, color = "icon-weak" }),
    })
    if iconUrl ~= nil and iconUrl ~= "" then
        return ui.image({ src = iconUrl, w = 20, h = 20, radius = 4, fit = "contain" }, fallbackNode)
    end
    return fallbackNode
end

local function verdictLabel(i18n: I18nService, level: string): string
    if level == "supported" then
        return i18n:T("review.works")
    elseif level == "partial" then
        return i18n:T("review.partial")
    end
    return i18n:T("extensions.compat.unsupported")
end

local function accessLabel(i18n: I18nService, item: string): string
    local key = `extensions.access.{item}`
    local text = i18n:T(key)
    return if text ~= key then text else item
end

local function renderInstalledRow(env: SectionEnv, view: View, ext: ProfileExtension): ui.Node
    local i18n = env.I18n
    local extId = ext.id
    local compat = ext.compatibility
    local errText: string? = if ext.changed then i18n:T("settings.extensions.changed") else ext.error

    local subNode: ui.Node? = nil
    if errText ~= nil and errText ~= "" then
        subNode = ui.text(errText, DANGER_LINE, { role = "alert" })
    else
        local metaItems: { ui.Node } = {}
        local hasVersion = ext.version ~= nil and ext.version ~= ""
        if hasVersion then
            table.insert(metaItems, ui.text(ext.version, BODY_SINGLE))
        end
        if compat ~= nil and compat.level ~= "supported" then
            if hasVersion then
                table.insert(metaItems, ui.text("·", BODY_SINGLE))
            end
            local label = verdictLabel(i18n, compat.level)
            local style = if compat.level == "unsupported" then WARN_LINE else BODY_SINGLE
            table.insert(metaItems, ui.text(label, style, {
                help = compatTooltip(i18n, compat),
            }))
        end
        if #metaItems > 0 then
            subNode = ui.row({ gap = 6, align = "center", wrap = true }, metaItems)
        end
    end

    local textColItems: { ui.Node } = {
        ui.text(ext.name, TITLE),
    }
    if subNode ~= nil then
        table.insert(textColItems, subNode)
    end

    return ui.row({
        key = `settings-ext-row-{extId}`,
        w = "full",
        py = 20,
        gap = 16,
        align = "center",
    }, {
        ui.row({ grow = 1, shrink = true, minW = 0, gap = 12, align = "center" }, {
            glyph(ext.icon, "puzzle"),
            ui.col({ grow = 1, shrink = true, minW = 0, gap = 4 }, textColItems),
        }),
        ui.row({ gap = 8, align = "center" }, {
            ui.iconButton({
                key = `settings-ext-remove-{extId}`,
                icon = "outline-trash",
                variant = "ghost-muted",
                size = "small",
                label = i18n:T("settings.extensions.remove", { name = ext.name }),
                help = i18n:T("settings.extensions.remove", { name = ext.name }),
                disabled = env.busy,
                onPress = view:Handler(function()
                    env.onRemove(extId)
                end),
            }),
            ui.switch({
                key = `settings-ext-toggle-{extId}`,
                label = i18n:T("settings.extensions.enable", { name = ext.name }),
                checked = ext.enabled == true,
                disabled = env.busy or ext.changed == true,
                onChange = view:Handler(function(event)
                    env.onToggle(extId, (event :: any).checked == true)
                end),
            }),
        }),
    })
end

-- Renders Settings > Browser > Extensions section (`settings.tsx:130-153`).
function extensions.renderSettingsSection(env: SectionEnv, view: View): ui.Node
    local i18n = env.I18n
    local rows: { ui.Node } = {}
    for _, ext in env.list do
        table.insert(rows, renderInstalledRow(env, view, ext))
    end
    table.insert(rows, ui.settingsRow(
        {
            id = "settings-browser-extensions",
            title = i18n:T("settings.extensions.add.title"),
            description = i18n:T("settings.extensions.add.description"),
        },
        ui.button({
            key = "settings-browser-extensions-add",
            text = i18n:T("settings.extensions.add"),
            icon = "plus",
            size = "small",
            variant = "neutral",
            disabled = not env.available,
            onPress = view:Handler(function()
                env.onOpenAdd()
            end),
        })
    ))
    return ui.col({ key = "settings-browser-extensions-section", w = "full", gap = 16 }, {
        ui.settingsList({
            key = "settings-browser-extensions-list",
            title = i18n:T("settings.extensions.title"),
        }, rows),
        ui.text(i18n:T("settings.extensions.note"), NOTE),
    })
end

local function renderInstallReview(env: DialogEnv, view: View, rev: InstallReview): ui.Node
    local i18n = env.I18n
    local st = env.state
    local hardBlocked = extensions.isHardBlocked(rev.compatibility)
    local access: { string } = (rev.grants and rev.grants.access) or {}
    local permissions: { string } = (rev.grants and rev.grants.permissions) or {}
    local extConnect: { string } = (rev.grants and rev.grants.externally_connectable) or {}
    local webAccessible: { string } = (rev.grants and rev.grants.web_accessible_resources) or {}
    local sites: { string } = rev.sites or {}
    local hasAllSites = false
    for _, item in access do
        if item == "allSites" then
            hasAllSites = true
            break
        end
    end

    local function hasPerm(list: { string }, name: string): boolean
        for _, p in list do
            if p == name then
                return true
            end
        end
        return false
    end

    local body: { ui.Node } = {}

    local accessItems: { string } = {}
    for _, item in access do
        table.insert(accessItems, item)
    end
    if hasPerm(permissions, "scripting") then
        table.insert(accessItems, "scripting")
    end
    if hasPerm(permissions, "offscreen") then
        table.insert(accessItems, "offscreen")
    end

    if #accessItems > 0 then
        local bullets: { ui.Node } = {}
        for idx, item in accessItems do
            table.insert(bullets, ui.text(`•  {accessLabel(i18n, item)}`, BODY, { key = `rev-access-{idx}` }))
        end
        table.insert(body, ui.col({ key = "rev-access-group", w = "full", gap = 4 }, {
            ui.text(i18n:T("extensions.access.title"), STRONG_LINE),
            ui.col({ w = "full", pl = 6, gap = 2 }, bullets),
        }))
    end

    if #sites > 0 and not hasAllSites then
        local limit = if st.showAllSites then #sites else math.min(#sites, 5)
        local siteLines: { ui.Node } = {}
        for i = 1, limit do
            table.insert(siteLines, ui.text(`•  {sites[i]}`, BODY, { key = `rev-site-{i}`, dir = "ltr" }))
        end
        if #sites > 5 then
            local rem = #sites - 5
            local toggleLabel = if st.showAllSites
                then i18n:T("review.less")
                else i18n:T(if rem == 1 then "review.more.one" else "review.more.other", { count = rem })
            table.insert(siteLines, ui.button({
                key = "rev-sites-more",
                text = toggleLabel,
                size = "small",
                variant = "ghost-muted",
                onPress = view:Handler(function()
                    env.onToggleSites()
                end),
            }))
        end
        table.insert(body, ui.col({ key = "rev-sites-group", w = "full", gap = 4 }, {
            ui.text(i18n:T("review.sites"), STRONG_LINE),
            ui.col({ w = "full", pl = 6, gap = 2 }, siteLines),
        }))
    end

    if #permissions > 0 then
        local permLines: { ui.Node } = {}
        local shown = math.min(#permissions, 10)
        for i = 1, shown do
            table.insert(permLines, ui.text(`•  {permissions[i]}`, BODY, { key = `rev-perm-{i}`, dir = "ltr" }))
        end
        if #permissions > shown then
            local rem = #permissions - shown
            table.insert(permLines, ui.text(i18n:T(if rem == 1 then "review.more.one" else "review.more.other", { count = rem }), MUTED_META, { key = "rev-perm-more" }))
        end
        table.insert(body, ui.col({ key = "rev-perms-group", w = "full", gap = 4 }, {
            ui.text(i18n:T("review.permissions"), STRONG_LINE),
            ui.col({ w = "full", pl = 6, gap = 2 }, permLines),
        }))
    end

    if #extConnect > 0 then
        local lines: { ui.Node } = {}
        local shown = math.min(#extConnect, 5)
        for i = 1, shown do
            table.insert(lines, ui.text(`•  {extConnect[i]}`, BODY, { key = `rev-extconn-{i}`, dir = "ltr" }))
        end
        if #extConnect > shown then
            local rem = #extConnect - shown
            table.insert(lines, ui.text(i18n:T(if rem == 1 then "review.more.one" else "review.more.other", { count = rem }), MUTED_META, { key = "rev-extconn-more" }))
        end
        table.insert(body, ui.col({ key = "rev-extconn-group", w = "full", gap = 4 }, {
            ui.text(i18n:T("review.externalConnect"), STRONG_LINE),
            ui.col({ w = "full", pl = 6, gap = 2 }, lines),
        }))
    end

    if #webAccessible > 0 then
        local lines: { ui.Node } = {}
        local shown = math.min(#webAccessible, 5)
        for i = 1, shown do
            table.insert(lines, ui.text(`•  {webAccessible[i]}`, BODY, { key = `rev-war-{i}`, dir = "ltr" }))
        end
        if #webAccessible > shown then
            local rem = #webAccessible - shown
            table.insert(lines, ui.text(i18n:T(if rem == 1 then "review.more.one" else "review.more.other", { count = rem }), MUTED_META, { key = "rev-war-more" }))
        end
        table.insert(body, ui.col({ key = "rev-war-group", w = "full", gap = 4 }, {
            ui.text(i18n:T("review.webAccessible"), STRONG_LINE),
            ui.col({ w = "full", pl = 6, gap = 2 }, lines),
        }))
    end

    if rev.added ~= nil then
        local addedAccess: { string } = rev.added.access or {}
        local addedHosts: { string } = rev.added.hosts or {}
        local addedPerms: { string } = rev.added.permissions or {}
        local addedBullets: { ui.Node } = {}
        local mentionsSites = false
        for idx, item in addedAccess do
            if item == "someSites" or item == "allSites" then
                mentionsSites = true
                table.insert(addedBullets, ui.text(`•  {i18n:T("review.changeSites")}`, BODY, { key = `rev-add-{idx}` }))
            else
                table.insert(addedBullets, ui.text(`•  {accessLabel(i18n, item)}`, BODY, { key = `rev-add-{idx}` }))
            end
        end
        if hasPerm(addedPerms, "scripting") then
            table.insert(addedBullets, ui.text(`•  {accessLabel(i18n, "scripting")}`, BODY, { key = "rev-add-scripting" }))
        end
        if hasPerm(addedPerms, "offscreen") then
            table.insert(addedBullets, ui.text(`•  {accessLabel(i18n, "offscreen")}`, BODY, { key = "rev-add-offscreen" }))
        end
        if #addedHosts > 0 and not mentionsSites then
            table.insert(addedBullets, ui.text(`•  {i18n:T("review.moreSites")}`, BODY, { key = "rev-add-hosts" }))
        end
        if #addedBullets == 0 then
            table.insert(addedBullets, ui.text(`•  {i18n:T("review.morePermissions")}`, BODY, { key = "rev-add-perms" }))
        end
        table.insert(body, ui.col({ key = "rev-added-group", w = "full", gap = 4 }, {
            ui.text(i18n:T("review.new"), WARN_LABEL),
            ui.col({ w = "full", pl = 6, gap = 2 }, addedBullets),
        }))
    end

    local hasCompatWarn = rev.compatibility ~= nil and rev.compatibility.level ~= "supported"
    if hasCompatWarn then
        local dotColor: Color = if rev.compatibility.level == "unsupported" then "status-danger" else "status-warning"
        local compatBlock: { ui.Node } = {
            ui.row({
                key = "rev-verdict",
                gap = 8,
                align = "center",
                help = compatTooltip(i18n, rev.compatibility),
            }, {
                ui.box({ w = 6, h = 6, radius = 3, fill = dotColor }),
                ui.text(verdictLabel(i18n, rev.compatibility.level), STRONG_LINE),
            }),
        }
        if hardBlocked and rev.compatibility.cause ~= nil then
            local ck = `extensions.compat.cause.{rev.compatibility.cause}`
            table.insert(compatBlock, ui.text(i18n:T(ck), DANGER_LINE, { role = "alert" }))
        elseif rev.compatibility.missing ~= nil and #rev.compatibility.missing > 0 then
            local names: { string } = {}
            for _, feat in rev.compatibility.missing do
                local fk = `extensions.compat.feature.{feat}`
                local label = i18n:T(fk)
                if label == fk and feat == "management" then
                    label = "Managing other extensions"
                end
                table.insert(names, if label ~= fk then label else feat)
            end
            table.insert(
                compatBlock,
                ui.text(
                    `{i18n:T("extensions.compat.missing")} {table.concat(names, ", ")}`,
                    MUTED_LINE,
                    { key = "rev-compat-missing" }
                )
            )
        end
        table.insert(body, ui.col({ key = "rev-compat-group", w = "full", gap = 4 }, compatBlock))
    end

    local unvettedKey: string? = if rev.source == "folder"
        then "review.source.folder"
        elseif rev.source == "browser" then "review.source.browser"
        else nil
    if unvettedKey ~= nil then
        table.insert(body, ui.text(i18n:T(unvettedKey), BODY, { key = "rev-unvetted" }))
    end

    if rev.replaces ~= nil and rev.replaces ~= "" then
        table.insert(body, ui.text(i18n:T("settings.extensions.replaces", { name = rev.replaces }), BODY, { key = "rev-replaces" }))
    end

    if #body == 0 then
        table.insert(body, ui.text(i18n:T("review.summary.none"), BODY, { key = "rev-empty" }))
    end

    local token = rev.token
    local blocked = st.approving
    local confirmText = if rev.unsupported
        then i18n:T("settings.extensions.anyway")
        else i18n:T("settings.extensions.add")

    return ui.col({ key = "ext-review-dialog", w = "full" }, {
        ui.row({ key = "ext-review-header", w = "full", px = 16, pt = 16, pb = 12, gap = 10, align = "center" }, {
            glyph(rev.icon, "puzzle"),
            ui.text(i18n:T("settings.extensions.confirm.title", { name = rev.name }), DIALOG_TITLE, {
                grow = 1,
                shrink = true,
                minW = 0,
            }),
            ui.iconButton({
                key = "ext-review-close",
                icon = "close",
                variant = "ghost-muted",
                size = "small",
                label = i18n:T("common.close"),
                onPress = view:Handler(function()
                    env.onClose()
                end),
            }),
        }),
        ui.col({
            key = "ext-review-body",
            w = "full",
            px = 16,
            py = 4,
            gap = 12,
            maxH = 340,
            scrollY = true,
        }, body),
        ui.row({ key = "ext-review-footer", w = "full", px = 16, py = 16, justify = "end", gap = 8 }, {
            ui.button({
                key = "ext-review-cancel",
                text = i18n:T("common.cancel"),
                variant = "neutral",
                disabled = blocked,
                onPress = view:Handler(function()
                    env.onCancelReview()
                end),
            }),
            ui.button({
                key = "ext-review-approve",
                text = confirmText,
                variant = "contrast",
                disabled = blocked or hardBlocked,
                loading = st.approving,
                onPress = view:Handler(function()
                    env.onApprove(token)
                end),
            }),
        }),
    })
end

-- Renders the "Add an extension" dialog, switching cleanly into InstallReview when confirming (`settings.tsx:432-720`).
function extensions.renderAddDialog(env: DialogEnv, view: View): ui.Node
    local st = env.state
    if st.review ~= nil then
        return renderInstallReview(env, view, st.review)
    end

    local i18n = env.I18n
    local blocked = st.busy ~= nil
    local linkEmpty = string.match(env.linkEditor:Text(), "^%s*$") ~= nil
    local selected = extensions.findChoice(st.choices, st.selectedKey)

    local middle: { ui.Node } = {}

    if st.message ~= nil then
        local msg = st.message
        local style = if msg.error then DANGER_LINE else BODY
        table.insert(middle, ui.text(msg.text, style, {
            key = "ext-dialog-message",
            role = if msg.error then "alert" else "status",
            help = compatTooltip(i18n, msg.compatibility),
        }))
    end

    if selected ~= nil then
        local headerChildren: { ui.Node } = {
            ui.text(i18n:T("settings.extensions.browser.title"), SECTION_LABEL),
        }
        if #st.choices > 1 then
            table.insert(headerChildren, ui.row({
                key = "ext-browser-profile-btn",
                role = "button",
                label = i18n:T("settings.extensions.browser.select"),
                h = 24,
                px = 6,
                gap = 4,
                radius = 4,
                align = "center",
                hover = "surface-hover",
                press = "surface-press",
                hoverTransition = "instant",
                menuTrigger = true,
                onPress = view:Handler(function()
                    env.onOpenProfileMenu()
                end),
            }, {
                ui.text(extensions.choiceLabel(selected), SECTION_LABEL),
                ui.icon("chevron-down", { size = 14, color = "icon-weak" }),
            }))
        end

        local listItems: { ui.Node } = {}
        if #st.candidates == 0 then
            if st.candidatesLoaded then
                table.insert(listItems, ui.text(i18n:T("settings.extensions.browser.empty"), BODY, { key = "ext-browser-empty" }))
            end
        else
            local srcId = selected.sourceId
            local profId = selected.profileId
            for _, item in st.candidates do
                local extId = item.id
                local compat = item.compatibility
                local textItems: { ui.Node } = { ui.text(item.name, TITLE) }
                if compat ~= nil and compat.level ~= "supported" then
                    local noteStyle = if compat.level == "unsupported" then SUB_WARN else SUB_MUTED
                    table.insert(textItems, ui.text(verdictLabel(i18n, compat.level), noteStyle, {
                        help = compatTooltip(i18n, compat),
                    }))
                end
                local btnText = if item.installed
                    then i18n:T("settings.extensions.added")
                    elseif compat ~= nil and compat.level == "unsupported" then i18n:T("settings.extensions.anyway")
                    else i18n:T("settings.extensions.add")
                local btnVariant: ButtonVariant = if item.installed then "ghost-muted" else "neutral"
                local isBusyThis = st.busy == "install-browser" and st.installingId == extId
                table.insert(listItems, ui.row({
                    key = `ext-cand-{extId}`,
                    w = "full",
                    minH = 44,
                    py = 6,
                    gap = 12,
                    align = "center",
                }, {
                    glyph(item.icon, "puzzle"),
                    ui.col({ grow = 1, shrink = true, minW = 0, gap = 2 }, textItems),
                    ui.button({
                        key = `ext-cand-btn-{extId}`,
                        text = btnText,
                        size = "small",
                        minW = 96,
                        variant = btnVariant,
                        disabled = blocked or item.installed,
                        loading = isBusyThis,
                        onPress = view:Handler(function()
                            env.onAddFromBrowser(srcId, profId, extId)
                        end),
                    }),
                }))
            end
        end

        table.insert(middle, ui.col({ key = "ext-from-browsers", w = "full", gap = 4 }, {
            ui.row({ w = "full", h = 28, align = "center", justify = "between", gap = 8 }, headerChildren),
            ui.col({ w = "full", gap = 0 }, listItems),
        }))
    end

    local devChildren: { ui.Node } = {
        ui.row({
            key = "ext-dev-toggle",
            role = "button",
            label = i18n:T("settings.developers"),
            gap = 4,
            py = 2,
            radius = 4,
            align = "center",
            hover = "surface-hover",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                env.onToggleDev()
            end),
        }, {
            ui.text(i18n:T("settings.developers"), BODY_SINGLE),
            ui.icon(if st.devOpen then "chevron-up" else "chevron-down", { size = 14, color = "icon-weak" }),
        }),
    }
    if st.devOpen then
        table.insert(devChildren, ui.row({ key = "ext-dev-body", w = "full", pt = 4, gap = 12, align = "center" }, {
            ui.text(i18n:T("settings.extensions.folder.description"), BODY, { grow = 1, shrink = true, minW = 0 }),
            ui.button({
                key = "ext-dev-folder-btn",
                text = i18n:T("settings.extensions.folder.action"),
                size = "small",
                variant = "neutral",
                disabled = blocked,
                loading = st.busy == "install-folder",
                onPress = view:Handler(function()
                    env.onPickFolder()
                end),
            }),
        }))
    end

    local bodyChildren: { ui.Node } = {
        ui.row({ key = "ext-store-row", w = "full", align = "center", gap = 8 }, {
            ui.field({
                key = "ext-store-input",
                editor = env.linkEditor,
                label = i18n:T("settings.extensions.store.label"),
                placeholder = i18n:T("settings.extensions.store.placeholder"),
                size = "large",
                grow = 1,
                shrink = true,
                minW = 0,
                disabled = blocked,
                onSubmit = view:Handler(function()
                    env.onAddLink()
                end),
            }),
            ui.button({
                key = "ext-store-add-btn",
                text = i18n:T("settings.extensions.add"),
                size = "large",
                variant = "neutral",
                disabled = blocked or linkEmpty,
                loading = st.busy == "install-store",
                onPress = view:Handler(function()
                    env.onAddLink()
                end),
            }),
        }),
    }

    if #middle > 0 then
        table.insert(bodyChildren, ui.col({
            key = "ext-dialog-scroll",
            w = "full",
            maxH = 280,
            scrollY = true,
            gap = 12,
        }, middle))
    end

    table.insert(bodyChildren, ui.col({ key = "ext-dev-section", w = "full", gap = 8 }, {
        ui.divider(),
        ui.col({ w = "full", pt = 4, gap = 4 }, devChildren),
    }))

    return ui.col({ key = "ext-add-dialog", w = "full" }, {
        ui.dialogHeader({
            key = "ext-add-header",
            title = i18n:T("settings.extensions.add.title"),
            description = i18n:T("settings.extensions.dialog.description"),
            wrapTitle = false,
        }),
        ui.col({ key = "ext-add-body", w = "full", px = 16, pt = 4, pb = 8, gap = 16 }, bodyChildren),
        ui.row({ key = "ext-add-footer", w = "full", px = 16, py = 12, justify = "end" }, {
            ui.button({
                key = "ext-add-done",
                text = i18n:T("settings.extensions.done"),
                variant = "contrast",
                onPress = view:Handler(function()
                    env.onClose()
                end),
            }),
        }),
    })
end

function extensions.renderProfileMenu(
    choices: { ProfileChoice },
    selectedKey: string?,
    view: View,
    onSelect: (key: string) -> ()
): ui.Node
    local rows: { ui.Node } = {}
    for idx, choice in choices do
        local key = extensions.choiceKey(choice)
        local active = key == selectedKey or (selectedKey == nil and idx == 1)
        table.insert(rows, ui.row({
            key = `ext-prof-opt-{idx}`,
            role = "button",
            label = extensions.choiceLabel(choice),
            w = "full",
            minH = 28,
            px = 10,
            py = 4,
            radius = 4,
            align = "center",
            selected = active,
            hover = "surface-hover",
            press = "surface-press",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                onSelect(key)
            end),
        }, {
            ui.text(extensions.choiceLabel(choice), TITLE),
        }))
    end
    return ui.col({ key = "ext-profile-popover", minW = 180, pad = 4, gap = 2 }, rows)
end

export type ToolbarAction = {
    id: string,
    name: string,
    title: string?,
    icon: string?,
    badge: string?,
    badgeColor: string?,
    badgeTextColor: string?,
    enabled: boolean,
    popup: string?,
    options: string?,
}

-- Toolbar puzzle popover (`panel.tsx:1174-1205`): 220 pt wide, 16 pt icon, truncated title,
-- optional badge pill, hairline separator, and "Manage extensions".
function extensions.renderToolbarMenu(
    items: { ToolbarAction },
    i18n: I18nService,
    view: View,
    onSelect: (item: ToolbarAction) -> (),
    onManage: () -> ()
): ui.Node
    local rows: { ui.Node } = {}
    for _, item in items do
        local label = item.title or item.name
        local enabled = item.enabled ~= false
        local iconNode: ui.Node = if item.icon and item.icon ~= ""
            then ui.image({ src = item.icon, w = 16, h = 16, radius = 3, fit = "contain", shrink = false })
            else ui.icon("puzzle", { size = 16, color = if enabled then "text-muted" else "text-faint" })
        local rowChildren: { ui.Node } = {
            ui.row({ w = 16, h = 16, center = true, shrink = false }, { iconNode }),
            ui.text(label, {
                size = 13,
                weight = "regular",
                lineHeight = 16,
                tracking = -0.04,
                color = if enabled then "text-chrome" else "text-faint",
                truncate = true,
                grow = 1,
                shrink = true,
                minW = 0,
            } :: UiTextProps),
        }
        if item.badge and item.badge ~= "" then
            table.insert(rowChildren, ui.row({
                px = 4,
                h = 14,
                radius = 3,
                center = true,
                shrink = false,
                fill = if item.badgeColor then "accent" else "surface-2",
            }, {
                ui.text(item.badge, {
                    size = 10,
                    weight = "medium",
                    lineHeight = 14,
                    color = if item.badgeColor then "text-on-accent" else "text-muted",
                    noWrap = true,
                } :: any),
            }))
        end
        table.insert(rows, ui.row({
            key = `browser-ext-item-{item.id}`,
            role = "button",
            label = label,
            w = "full",
            h = 28,
            px = 8,
            gap = 8,
            radius = 4,
            align = "center",
            disabled = not enabled,
            hover = if enabled then "surface-hover" else nil,
            press = if enabled then "surface-press" else nil,
            hoverTransition = "instant",
            onPress = if enabled
                then view:Handler(function()
                    onSelect(item)
                end)
                else nil,
        }, rowChildren))
    end
    table.insert(rows, ui.col({ w = "full", h = 1, my = 2, fill = "border-weak" }))
    table.insert(rows, ui.row({
        key = "browser-ext-manage",
        role = "button",
        label = i18n:T("extensions.manage"),
        w = "full",
        h = 28,
        px = 8,
        radius = 4,
        align = "center",
        hover = "surface-hover",
        press = "surface-press",
        hoverTransition = "instant",
        onPress = view:Handler(function()
            onManage()
        end),
    }, {
        ui.text(i18n:T("extensions.manage"), {
            size = 13,
            weight = "regular",
            lineHeight = 16,
            tracking = -0.04,
            color = "text-chrome",
            truncate = true,
            grow = 1,
            shrink = true,
            minW = 0,
        } :: UiTextProps),
    }))
    return ui.col({ key = "browser-ext-menu", w = 220, pad = 4, gap = 2 }, rows)
end

return extensions
