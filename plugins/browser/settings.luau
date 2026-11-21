--!strict
-- Settings > Browser matching upstream `packages/gui-plugins/src/browser/settings.tsx`,
-- `settings-view.ts`, `UP-108` (#65095), and `UP-111` (#65101):
-- - Persists user preferences under `plugin.browser/preferences` without altering existing keys
--   (`history`, `rune.siteZoom`, `rune.sitePermissions`).
-- - Capability-gates sections whose engine side is owned by WS-1 / WS-9 (`import`, `extensions`,
--   `remoteSignIns`, `cookieFile`) so nothing broken or stubbed is ever shown.
-- - Collapsible `Advanced` section automatically expands when Settings search targets an entry
--   inside it, without mutating the stored `advanced` preference.

local ui = require("@opencode/ui")
local hostDisplay = require("./host_display")

local settings = {}

export type Preferences = {
    links: boolean,
    agentPages: boolean,
    advanced: boolean,
    welcomed: boolean,
}

export type SiteDecision = {
    origin: string,
    permission: string,
    setting: "allow" | "block",
}

export type Capabilities = {
    available: boolean,
    import: boolean,
    extensions: boolean,
    remoteSignIns: boolean,
    cookieFile: boolean,
}

export type ImportProfile = {
    id: string,
    name: string,
}

export type ImportBrowser = {
    id: string,
    name: string,
    engine: string,
    profiles: { ImportProfile },
}

export type Env = {
    I18n: I18nService,
    preferences: () -> Preferences,
    updatePreferences: (patch: { [string]: any }) -> (),
    capabilities: () -> Capabilities,
    sitePermissions: () -> { SiteDecision },
    permissionRestartNote: (() -> boolean)?,
    setSitePermission: (origin: string, permission: string, setting: "allow" | "block") -> (),
    removeSitePermission: (origin: string, permission: string) -> (),
    historyCount: () -> number,
    clearNotice: () -> string?,
    requestClearBrowsingData: () -> (),
    openImportHistory: () -> (),
    -- Optional contract-backed sections provided by WS-9 (extensions) or WS-1 (cookies/remote).
    renderImport: ((view: View) -> ui.Node?)?,
    renderExtensions: ((view: View) -> ui.Node?)?,
    remoteSignInsEnabled: (() -> boolean)?,
    setRemoteSignIns: ((enabled: boolean) -> ())?,
    pickCookieFile: (() -> ())?,
}

-- Search entries whose rows live inside the collapsible Advanced disclosure (`settings-view.ts`).
local ADVANCED_ENTRIES: { [string]: boolean } = {
    ["settings-browser-remote"] = true,
    ["settings-browser-agent-pages"] = true,
    ["settings-browser-sites"] = true,
    ["settings-browser-cookies"] = true,
}

local DEFAULT_PREFS: Preferences = table.freeze({
    links = true,
    agentPages = true,
    advanced = false,
    welcomed = false,
})

function settings.normalizePreferences(raw: any): Preferences
    if type(raw) ~= "table" then
        return table.clone(DEFAULT_PREFS)
    end
    return {
        links = raw.links ~= false,
        agentPages = raw.agentPages ~= false,
        advanced = raw.advanced == true,
        welcomed = raw.welcomed == true,
    }
end

function settings.advancedOpen(stored: boolean, target: string?, closedOver: string?): boolean
    if stored then
        return true
    end
    if target ~= nil and target ~= closedOver and ADVANCED_ENTRIES[target] == true then
        return true
    end
    return false
end

-- Builds Settings search entries reflecting only the currently active capabilities.
function settings.entries(i18n: I18nService, caps: Capabilities): { SettingEntry }
    local out: { SettingEntry } = {
        {
            id = "settings-browser-links",
            title = i18n:T("settings.links.title"),
            description = i18n:T("settings.links.description"),
            keywords = "links open system default browser opencode's browser web shift click",
        },
        {
            id = "settings-browser-agent-pages",
            title = i18n:T("settings.agentPages.title"),
            description = i18n:T("settings.agentPages.description"),
            keywords = "open the browser when the agent shows a page agent preview automatic browser pane tab file",
        },
        {
            id = "settings-browser-sites",
            title = i18n:T("settings.sites.title"),
            keywords = "site permissions camera microphone location notifications clipboard",
        },
        {
            id = "settings-browser-cookies",
            title = i18n:T("settings.cookies.title"),
            keywords = "cookies sign out browsing data history clear",
        },
    }
    if caps.import then
        table.insert(out, {
            id = "settings-browser-import",
            title = i18n:T("settings.import.title"),
            keywords = "import cookies history chrome edge firefox safari helium arc brave sign in",
        })
    end
    if caps.extensions then
        table.insert(out, {
            id = "settings-browser-extensions",
            title = i18n:T("settings.extensions.title"),
            keywords = "extensions",
        })
    end
    if caps.remoteSignIns then
        table.insert(out, {
            id = "settings-browser-remote",
            title = i18n:T("settings.remote.title"),
            description = i18n:T("settings.remote.description"),
            keywords = "remote server sign in cookies share",
        })
    end
    return out
end

local NOTE: UiTextProps = table.freeze({ size = 12, weight = "regular", lineHeight = 16, color = "text-faint", wrap = true } :: UiTextProps)
local MUTED: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 18, color = "text-muted", wrap = true } :: UiTextProps)
local HEADING: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", noWrap = true } :: UiTextProps)

local function advancedSummary(i18n: I18nService, caps: Capabilities): string
    if caps.remoteSignIns and caps.cookieFile then
        return i18n:T("settings.advanced.summary")
    end
    local parts: { string } = {}
    if caps.remoteSignIns then
        table.insert(parts, "Remote servers")
    end
    table.insert(parts, i18n:T("settings.sites.title"))
    table.insert(parts, i18n:T("settings.data.title"))
    if caps.cookieFile then
        table.insert(parts, "cookie file")
    end
    return table.concat(parts, ", ")
end

local function permissionName(i18n: I18nService, kind: string): string
    local key = `settings.sites.permission.{kind}`
    local translated = i18n:T(key)
    return if translated ~= key then translated else kind
end

-- Stateful page renderer holding ephemeral visit state (`closedOver` search target).
function settings.create(env: Env, refresh: () -> ())
    local closedOver: string? = nil

    local function render(view: View, props: SettingsPageProps): ui.Node
        local i18n = env.I18n
        local caps = env.capabilities()
        if not caps.available then
            return ui.col({ key = "settings-browser-page", w = "full", gap = 16 }, {
                ui.settingsList({ key = "settings-browser-unavailable" }, {
                    ui.row({ w = "full", px = 16, py = 12 }, {
                        ui.text(i18n:T("settings.unavailable"), MUTED),
                    }),
                }),
            })
        end

        local prefs = env.preferences()
        local target = props.target
        local isAdvanced = settings.advancedOpen(prefs.advanced, target, closedOver)
        local sections: { ui.Node } = {}

        -- 1. Import from another browser (user-explicit read-only history import + optional WS-1 cookie import)
        if caps.import then
            local customImport = if env.renderImport then env.renderImport(view) else nil
            if customImport then
                table.insert(sections, customImport)
            else
                table.insert(sections, ui.settingsList({ key = "settings-browser-import-section" }, {
                    ui.settingsRow(
                        {
                            id = "settings-browser-import",
                            title = i18n:T("settings.import.title"),
                            description = i18n:T("settings.import.history.description"),
                        },
                        ui.button({
                            key = "settings-browser-import-btn",
                            text = i18n:T("settings.import.open"),
                            size = "small",
                            variant = "neutral",
                            onPress = view:Handler(function()
                                env.openImportHistory()
                            end),
                        })
                    ),
                }))
            end
        end

        -- 2. Extensions (shown when WS-9's extensions capability is active)
        if caps.extensions and env.renderExtensions then
            local extNode = env.renderExtensions(view)
            if extNode then
                table.insert(sections, extNode)
            end
        end

        -- 3. Open links in OpenCode (`settings-browser-links`)
        table.insert(sections, ui.settingsList({ key = "settings-browser-links-section" }, {
            ui.settingsRow(
                {
                    id = "settings-browser-links",
                    title = i18n:T("settings.links.title"),
                    description = i18n:T("settings.links.description"),
                },
                ui.switch({
                    key = "settings-browser-links-switch",
                    label = i18n:T("settings.links.title"),
                    checked = prefs.links,
                    onChange = view:Handler(function(event)
                        env.updatePreferences({ links = (event :: any).checked == true })
                    end),
                })
            ),
        }))

        -- 4. Collapsible Advanced disclosure heading (`settings.tsx:172-198`)
        local disclosureItems: { ui.Node } = {
            ui.text(i18n:T("settings.advanced"), HEADING),
        }
        if not isAdvanced then
            table.insert(disclosureItems, ui.text(advancedSummary(i18n, caps), {
                size = 13,
                weight = "regular",
                lineHeight = 18,
                color = "text-muted",
                ml = 8,
                truncate = true,
                shrink = true,
                minW = 0,
            }))
        end
        table.insert(disclosureItems, ui.icon(if isAdvanced then "chevron-down" else "chevron-right", {
            size = 14,
            color = "icon-weak",
            ml = 4,
        }))
        table.insert(sections, ui.row({
            key = "settings-browser-advanced",
            role = "button",
            label = i18n:T("settings.advanced"),
            w = "full",
            py = 6,
            px = 4,
            radius = 4,
            align = "center",
            hover = "surface-hover",
            press = "surface-press",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                local nextOpen = not isAdvanced
                closedOver = if nextOpen then nil else target
                env.updatePreferences({ advanced = nextOpen })
                refresh()
            end),
        }, disclosureItems))

        if isAdvanced then
            -- 4a. Behavior rows (Remote sign-in sharing if capable + Agent pages toggle)
            local advRows: { ui.Node } = {}
            if caps.remoteSignIns and env.remoteSignInsEnabled and env.setRemoteSignIns then
                table.insert(advRows, ui.settingsRow(
                    {
                        id = "settings-browser-remote",
                        title = i18n:T("settings.remote.title"),
                        description = i18n:T("settings.remote.description"),
                    },
                    ui.switch({
                        key = "settings-browser-remote-switch",
                        label = i18n:T("settings.remote.title"),
                        checked = env.remoteSignInsEnabled(),
                        onChange = view:Handler(function(event)
                            if env.setRemoteSignIns then
                                env.setRemoteSignIns((event :: any).checked == true)
                            end
                        end),
                    })
                ))
            end
            table.insert(advRows, ui.settingsRow(
                {
                    id = "settings-browser-agent-pages",
                    title = i18n:T("settings.agentPages.title"),
                    description = i18n:T("settings.agentPages.description"),
                },
                ui.switch({
                    key = "settings-browser-agent-pages-switch",
                    label = i18n:T("settings.agentPages.title"),
                    checked = prefs.agentPages,
                    onChange = view:Handler(function(event)
                        env.updatePreferences({ agentPages = (event :: any).checked == true })
                    end),
                })
            ))
            table.insert(sections, ui.settingsList({ key = "settings-browser-behavior-section" }, advRows))

            -- 4b. Cookie file import (gated on WS-1 Phase 3 capability)
            if caps.cookieFile and env.pickCookieFile then
                table.insert(sections, ui.settingsList({ key = "settings-browser-cookie-file-section" }, {
                    ui.settingsRow(
                        {
                            id = "settings-browser-cookie-file",
                            title = i18n:T("settings.import.file.title"),
                            description = i18n:T("settings.import.file.description"),
                        },
                        ui.button({
                            key = "settings-browser-cookie-file-btn",
                            text = i18n:T("settings.import.file.action"),
                            size = "small",
                            variant = "neutral",
                            onPress = view:Handler(function()
                                if env.pickCookieFile then
                                    env.pickCookieFile()
                                end
                            end),
                        })
                    ),
                }))
            end

            -- 4c. Site permissions section (`settings-browser-sites`)
            local decisions = env.sitePermissions()
            local siteRows: { ui.Node } = {}
            if #decisions == 0 then
                table.insert(siteRows, ui.settingsRow({
                    id = "settings-browser-sites",
                    title = i18n:T("settings.sites.none"),
                }))
            else
                for idx, d in decisions do
                    local origin = d.origin
                    local perm = d.permission
                    local setting = d.setting
                    local host = hostDisplay.displayHost(origin)
                    if host == "" then
                        host = origin
                    end
                    local permLabel = permissionName(i18n, perm)
                    local settingLabel = i18n:T(if setting == "allow" then "settings.sites.allow" else "settings.sites.block")
                    local nextSetting: "allow" | "block" = if setting == "allow" then "block" else "allow"
                    local controls = ui.row({ gap = 8, align = "center" }, {
                        ui.button({
                            key = `site-perm-toggle-{idx}`,
                            text = settingLabel,
                            label = i18n:T("settings.sites.setting", { permission = permLabel, site = host }),
                            size = "small",
                            variant = "neutral",
                            onPress = view:Handler(function()
                                env.setSitePermission(origin, perm, nextSetting)
                            end),
                        }),
                        ui.iconButton({
                            key = `site-perm-remove-{idx}`,
                            icon = "outline-trash",
                            variant = "ghost-muted",
                            size = "small",
                            label = i18n:T("settings.sites.remove", { permission = permLabel, site = host }),
                            help = i18n:T("settings.sites.remove", { permission = permLabel, site = host }),
                            onPress = view:Handler(function()
                                env.removeSitePermission(origin, perm)
                            end),
                        }),
                    })
                    table.insert(siteRows, ui.settingsRow(
                        {
                            id = if idx == 1 then "settings-browser-sites" else `settings-browser-site-{idx}`,
                            title = host,
                            description = permLabel,
                        },
                        controls
                    ))
                end
            end
            local sitesNotes: { ui.Node } = {
                ui.settingsList({ key = "settings-browser-sites-list", title = i18n:T("settings.sites.title") }, siteRows),
                ui.text(i18n:T("settings.sites.note"), NOTE),
            }
            if env.permissionRestartNote and env.permissionRestartNote() then
                table.insert(sitesNotes, ui.text(i18n:T("settings.sites.restartNote"), NOTE))
            end
            table.insert(sections, ui.col({ key = "settings-browser-sites-section", w = "full", gap = 16 }, sitesNotes))

            -- 4d. Browser data section (`settings-browser-cookies`, UP-117 #65124 confirm before clearing)
            local count = env.historyCount()
            local countText = if count > 0
                then i18n:T(if count == 1 then "settings.history.count.one" else "settings.history.count.other", { count = count })
                else nil
            local dataDesc = env.clearNotice() or countText
            table.insert(sections, ui.settingsList({ key = "settings-browser-data-section", title = i18n:T("settings.data.title") }, {
                ui.settingsRow(
                    {
                        id = "settings-browser-cookies",
                        title = i18n:T("settings.cookies.title"),
                        description = dataDesc,
                    },
                    ui.button({
                        key = "settings-browser-clear-data",
                        text = i18n:T("settings.cookies.clear"),
                        size = "small",
                        variant = "danger",
                        onPress = view:Handler(function()
                            env.requestClearBrowsingData()
                        end),
                    })
                ),
            }))
        end

        return ui.col({ key = "settings-browser-page", w = "full", gap = 32 }, sections)
    end

    return { render = render }
end

local DIALOG_TITLE: UiTextProps = table.freeze({ size = 15, weight = "medium", lineHeight = 20, color = "text-chrome", noWrap = true } :: UiTextProps)

-- Confirmation dialog body for Clear browsing data (`UP-117` `#65124`).
function settings.renderClearConfirm(i18n: I18nService, view: View, onCancel: () -> (), onConfirm: () -> ()): ui.Node
    return ui.col({ key = "settings-browser-clear-confirm", w = "full", pad = 20, gap = 12 }, {
        ui.text(i18n:T("settings.clear.confirm.title"), DIALOG_TITLE),
        ui.text(i18n:T("settings.clear.confirm.description"), MUTED),
        ui.row({ w = "full", justify = "end", gap = 8, pt = 4 }, {
            ui.button({
                key = "settings-browser-clear-cancel",
                text = i18n:T("common.cancel"),
                size = "small",
                variant = "ghost",
                autofocus = true,
                onPress = view:Handler(function()
                    onCancel()
                end),
            }),
            ui.button({
                key = "settings-browser-clear-confirm-btn",
                text = i18n:T("settings.clear.confirm.action"),
                size = "small",
                variant = "danger",
                onPress = view:Handler(function()
                    onConfirm()
                end),
            }),
        }),
    })
end

-- User-explicit read-only browser history importer dialog (`UP-106` `#65073`).
function settings.renderImportDialog(
    i18n: I18nService,
    view: View,
    sources: { ImportBrowser },
    statusText: string?,
    onImport: (browserId: string, profileId: string) -> (),
    onClose: () -> ()
): ui.Node
    local rows: { ui.Node } = {
        ui.row({ w = "full", align = "center", gap = 8 }, {
            ui.text(i18n:T("settings.import.title"), DIALOG_TITLE, { grow = 1, shrink = true, minW = 0, truncate = true }),
            ui.iconButton({
                key = "settings-import-x",
                icon = "close",
                variant = "ghost-muted",
                size = "small",
                label = i18n:T("common.close"),
                onPress = view:Handler(function()
                    onClose()
                end),
            }),
        }),
        ui.text(i18n:T("settings.import.history.description"), MUTED),
    }
    if #sources == 0 then
        table.insert(rows, ui.row({ key = "settings-import-none", w = "full", py = 12, px = 12, radius = 6, fill = "surface-1" }, {
            ui.text(i18n:T("settings.import.none"), MUTED),
        }))
    else
        local listItems: { ui.Node } = {}
        local idx = 0
        for _, browser in sources do
            for _, prof in browser.profiles do
                idx += 1
                local bid, pid = browser.id, prof.id
                local label = if #browser.profiles > 1 then `{browser.name} ({prof.name})` else browser.name
                table.insert(listItems, ui.row({
                    key = `import-source-{idx}`,
                    w = "full",
                    px = 12,
                    py = 8,
                    gap = 8,
                    radius = 6,
                    fill = "surface-1",
                    align = "center",
                }, {
                    ui.col({ grow = 1, shrink = true, minW = 0, gap = 2 }, {
                        ui.text(label, HEADING),
                        ui.text(prof.name, NOTE),
                    }),
                    ui.button({
                        key = `import-source-btn-{idx}`,
                        text = i18n:T("settings.import.history.action"),
                        size = "small",
                        variant = "neutral",
                        onPress = view:Handler(function()
                            onImport(bid, pid)
                        end),
                    }),
                }))
            end
        end
        table.insert(rows, ui.col({ key = "settings-import-list", w = "full", maxH = 260, scrollY = true, gap = 6 }, listItems))
    end
    if statusText ~= nil and statusText ~= "" then
        table.insert(rows, ui.text(statusText, { key = "settings-import-status", size = 12, weight = "medium", lineHeight = 16, color = "text-chrome", wrap = true }))
    end
    table.insert(rows, ui.row({ w = "full", justify = "end", gap = 8, pt = 4 }, {
        ui.button({
            key = "settings-import-close",
            text = i18n:T("common.close"),
            size = "small",
            variant = "ghost",
            onPress = view:Handler(function()
                onClose()
            end),
        }),
    }))
    return ui.col({ key = "settings-browser-import-dialog", w = "full", pad = 20, gap = 12 }, rows)
end

return settings
