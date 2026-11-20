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

export type Env = {
    I18n: I18nService,
    preferences: () -> Preferences,
    updatePreferences: (patch: { [string]: any }) -> (),
    capabilities: () -> Capabilities,
    sitePermissions: () -> { SiteDecision },
    setSitePermission: (origin: string, permission: string, setting: "allow" | "block") -> (),
    removeSitePermission: (origin: string, permission: string) -> (),
    clearBrowsingData: () -> boolean,
    -- Optional contract-backed sections provided by WS-9 (extensions) or PR 4 / WS-1 (import).
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

local NOTE = table.freeze({ size = 12, weight = "regular", lineHeight = 16, color = "text-muted", wrap = true })
local MUTED = table.freeze({ size = 13, weight = "regular", lineHeight = 18, color = "text-muted", wrap = true })
local HEADING = table.freeze({ size = 13, weight = "medium", lineHeight = 18, color = "text-chrome", noWrap = true })

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

-- Stateful page renderer holding ephemeral visit state (`closedOver` search target, clear status).
function settings.create(env: Env, refresh: () -> ())
    local closedOver: string? = nil
    local clearNotice: string? = nil

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

        -- 1. Import from another browser (shown when history or cookie import capability is active)
        if caps.import and env.renderImport then
            local importNode = env.renderImport(view)
            if importNode then
                table.insert(sections, importNode)
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
            table.insert(sections, ui.col({ key = "settings-browser-sites-section", w = "full", gap = 6 }, {
                ui.settingsList({ key = "settings-browser-sites-list", title = i18n:T("settings.sites.title") }, siteRows),
                ui.text(i18n:T("settings.sites.note"), NOTE),
            }))

            -- 4d. Browser data section (`settings-browser-cookies`)
            table.insert(sections, ui.settingsList({ key = "settings-browser-data-section", title = i18n:T("settings.data.title") }, {
                ui.settingsRow(
                    {
                        id = "settings-browser-cookies",
                        title = i18n:T("settings.cookies.title"),
                        description = clearNotice,
                    },
                    ui.button({
                        key = "settings-browser-clear-data",
                        text = i18n:T("settings.cookies.clear"),
                        size = "small",
                        variant = "danger",
                        onPress = view:Handler(function()
                            local ok = env.clearBrowsingData()
                            clearNotice = i18n:T(if ok then "settings.cleared" else "settings.failed.clear")
                            refresh()
                        end),
                    })
                ),
            }))
        end

        return ui.col({ key = "settings-browser-page", w = "full", gap = 20 }, sections)
    end

    return { render = render }
end

return settings
