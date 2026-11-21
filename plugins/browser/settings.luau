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
    -- `profile/index.ts:118` `remote-sign-ins`: off until the person turns it on.
    remoteSignIns: boolean,
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

-- How the last import from one browser profile ended, shown in its row (`settings.tsx` `Identity`).
export type ImportMessage = {
    text: string,
    error: boolean,
    -- Announced as it changes (an action's outcome), not a fixed detail.
    live: boolean?,
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
    clearNotice: () -> string?,
    requestClearBrowsingData: () -> (),
    importSources: () -> { ImportBrowser },
    importMessage: (browserId: string, profileId: string) -> ImportMessage?,
    openImport: (browserId: string, profileId: string) -> (),
    Menu: MenuService,
    -- Optional contract-backed sections provided by WS-9 (extensions) or WS-1 (cookies/remote).
    renderImport: ((view: View) -> ui.Node?)?,
    renderExtensions: ((view: View) -> ui.Node?)?,
    -- nil while the engine's cookie store is unknown (`settings.tsx:208-209`).
    remoteSignInsEnabled: (() -> boolean?)?,
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
    remoteSignIns = false,
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
        remoteSignIns = raw.remoteSignIns == true,
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

-- `settings.tsx` `Note`: 12 / 16 in the faint ink, under a section's card.
local NOTE: UiTextProps = table.freeze({ size = 12, weight = "regular", lineHeight = 16, color = "text-faint", wrap = true } :: UiTextProps)
local MUTED: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 20, color = "text-muted", wrap = true } :: UiTextProps)
-- `.settings-section-title`, as the Advanced heading's text.
local HEADING: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", noWrap = true } :: UiTextProps)
-- A settings row's title (`SettingsRow`), for rows that draw their own subject.
local ROW_TITLE: UiTextProps = table.freeze({ size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text-chrome", truncate = true } :: UiTextProps)
local ASIDE: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 16, color = "text-muted", truncate = true, shrink = true, minW = 0 } :: UiTextProps)
local SUBLINE: UiTextProps = table.freeze({ size = 13, weight = "regular", lineHeight = 20, color = "text-muted", wrap = true } :: UiTextProps)

-- Official's summary names the remote-server and cookie-file rows too; it is shown only once both exist.
local function advancedSummary(i18n: I18nService, caps: Capabilities): string
    if caps.remoteSignIns and caps.cookieFile then
        return i18n:T("settings.advanced.summary")
    end
    return i18n:T("settings.advanced.summary.local")
end

-- `choices.tsx` `Glyph`: a 20 pt box holding a muted 16 pt icon.
local function glyph(name: string): ui.Node
    return ui.row({ w = 20, h = 20, center = true, shrink = false }, {
        ui.icon(name, { size = 16, color = "icon-weak" }),
    })
end

-- `settings.tsx` `Identity` in a row of its own: the icon, the name with an optional muted aside on
-- its line, and under them a detail or what the last action did. Search reveals it by `id`.
local function identityRow(
    key: string,
    id: string?,
    icon: string,
    name: string,
    aside: string?,
    detail: ImportMessage?,
    control: ui.Node?
): ui.Node
    local line: { ui.Node } = { ui.text(name, ROW_TITLE, { shrink = true, minW = 0 }) }
    if aside ~= nil then
        table.insert(line, ui.text(aside, ASIDE))
    end
    local words: { ui.Node } = { ui.row({ gap = 6, align = "center", minW = 0 }, line) }
    if detail ~= nil then
        table.insert(words, ui.text(detail.text, SUBLINE, {
            role = if detail.error then "alert" elseif detail.live then "status" else nil,
            color = if detail.error then "status-danger" else "text-muted",
        }))
    end
    local items: { ui.Node } = {
        ui.row({ grow = 1, shrink = true, minW = 0, gap = 12, align = "center" }, {
            glyph(icon),
            ui.col({ grow = 1, shrink = true, minW = 0, gap = 4 }, words),
        }),
    }
    if control then
        table.insert(items, control)
    end
    return ui.row({ key = if id then `setting-{id}` else key, w = "full", gap = 16, py = 20, align = "center" }, items)
end

-- A row holding only muted words, as official's empty states (`Muted` titles).
local function mutedRow(id: string, words: string): ui.Node
    return ui.row({ key = `setting-{id}`, w = "full", py = 20 }, {
        ui.text(words, MUTED),
    })
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
                -- `settings.tsx:88-129`: one row per detected browser profile, the browser used last first.
                local rows: { ui.Node } = {}
                for _, browser in env.importSources() do
                    for _, prof in browser.profiles do
                        local idx = #rows + 1
                        local bid, pid = browser.id, prof.id
                        table.insert(rows, identityRow(
                            `import-source-{idx}`,
                            if idx == 1 then "settings-browser-import" else nil,
                            "outline-globe",
                            browser.name,
                            if #browser.profiles > 1 then prof.name else nil,
                            env.importMessage(bid, pid),
                            ui.button({
                                key = `import-source-btn-{idx}`,
                                text = i18n:T("settings.import.open"),
                                size = "small",
                                variant = "neutral",
                                onPress = view:Handler(function()
                                    env.openImport(bid, pid)
                                end),
                            })
                        ))
                    end
                end
                if #rows == 0 then
                    table.insert(rows, mutedRow("settings-browser-import", i18n:T("settings.import.none")))
                end
                table.insert(sections, ui.settingsList({
                    key = "settings-browser-import-section",
                    title = i18n:T("settings.import.title"),
                }, rows))
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
                lineHeight = 16,
                color = "text-muted",
                ml = 8,
                truncate = true,
                shrink = true,
                minW = 0,
            }))
        end
        -- `ADVANCED_CHEVRON`: the heading's own ink at 40%, ending the line's text.
        table.insert(disclosureItems, ui.icon(if isAdvanced then "outline-chevron-down" else "outline-chevron-right", {
            size = 16,
            color = "text-chrome",
            opacity = 0.4,
        }))
        -- `settings.tsx:171-197`: a section heading with no card and no fill that is a button.
        table.insert(sections, ui.row({
            key = "settings-browser-advanced",
            role = "button",
            label = i18n:T("settings.advanced"),
            w = "full",
            gap = 4,
            radius = 4,
            align = "center",
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
                -- `settings.tsx:200-212`: first row of the Advanced card.
                local shared = env.remoteSignInsEnabled()
                table.insert(advRows, ui.settingsRow(
                    {
                        id = "settings-browser-remote",
                        title = i18n:T("settings.remote.title"),
                        description = i18n:T("settings.remote.description"),
                    },
                    ui.switch({
                        key = "settings-browser-remote-switch",
                        label = i18n:T("settings.remote.title"),
                        checked = shared == true,
                        disabled = shared == nil,
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
                table.insert(siteRows, mutedRow("settings-browser-sites", i18n:T("settings.sites.none")))
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
                    local allowLabel = i18n:T("settings.sites.allow")
                    local blockLabel = i18n:T("settings.sites.block")
                    local selectKey = `site-perm-setting-{idx}`
                    local selectLabel = i18n:T("settings.sites.setting", { permission = permLabel, site = host })
                    -- `settings.tsx:339-351`: an inline `Select` (`SETTING_COLUMN`, 112 wide) of Allowed and
                    -- Blocked, drawn as the kit's inline trigger (24 tall, 8 / 4 in, value then chevron).
                    local controls = ui.row({ gap = 8, align = "center" }, {
                        ui.row({ w = 112, justify = "end" }, {
                            ui.row({
                                key = selectKey,
                                role = "button",
                                label = selectLabel,
                                h = 24,
                                py = 4,
                                pl = 8,
                                pr = 4,
                                gap = 4,
                                radius = 4,
                                align = "center",
                                hover = "surface-hover",
                                press = "surface-press",
                                hoverTransition = "instant",
                                shrink = true,
                                minW = 0,
                                menuTrigger = true,
                                onPress = view:Handler(function()
                                    local picked = env.Menu:OpenAsync({
                                        anchorKey = selectKey,
                                        label = selectLabel,
                                        align = "end",
                                        minWidth = 160,
                                        items = {
                                            { id = "allow", title = allowLabel, checked = setting == "allow" },
                                            { id = "block", title = blockLabel, checked = setting == "block" },
                                        },
                                    })
                                    if (picked == "allow" or picked == "block") and picked ~= setting then
                                        env.setSitePermission(origin, perm, picked :: any)
                                    end
                                end),
                            }, {
                                ui.text(if setting == "allow" then allowLabel else blockLabel, {
                                    size = 13,
                                    weight = "medium",
                                    lineHeight = 16,
                                    color = "text-chrome",
                                    truncate = true,
                                    shrink = true,
                                    minW = 0,
                                }),
                                ui.icon("select-chevron", { size = 16, color = "icon-weak" }),
                            }),
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
                    -- `settings.tsx:326-336`: the globe, the site, and the feature under it.
                    table.insert(siteRows, identityRow(
                        `site-perm-{idx}`,
                        if idx == 1 then "settings-browser-sites" else nil,
                        "outline-globe",
                        host,
                        nil,
                        { text = permLabel, error = false },
                        controls
                    ))
                end
            end
            -- `SettingsSection` footer: the notes sit 16 under the card, 4 apart.
            local notes: { ui.Node } = { ui.text(i18n:T("settings.sites.note"), NOTE) }
            if env.permissionRestartNote and env.permissionRestartNote() then
                table.insert(notes, ui.text(i18n:T("settings.sites.restartNote"), NOTE))
            end
            table.insert(sections, ui.col({ key = "settings-browser-sites-section", w = "full", gap = 16 }, {
                ui.settingsList({ key = "settings-browser-sites-list", title = i18n:T("settings.sites.title") }, siteRows),
                ui.col({ w = "full", gap = 4 }, notes),
            }))

            -- 4d. Browser data section (`settings-browser-cookies`, UP-117 #65124 confirm before clearing).
            -- Official's cookie count ("N cookies. Sign-ins are shared across sessions") needs the
            -- engine's profile cookie list, which WS-1's #286 adds; until then only the clear outcome shows.
            local dataDesc = env.clearNotice()
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

        -- `SettingsLayout`: sections 32 apart.
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

-- "Import from <browser>" (`site-chooser.tsx:328-347`) for what this build brings over: browsing
-- history, read-only from the other browser. A clean import closes it and its row says so; a
-- failed one stays open and says why.
function settings.renderImportDialog(
    i18n: I18nService,
    view: View,
    browserLabel: string,
    message: ImportMessage?,
    onImport: () -> (),
    onClose: () -> ()
): ui.Node
    local rows: { ui.Node } = {
        ui.text(i18n:T("signins.import", { browser = browserLabel }), DIALOG_TITLE, { truncate = true }),
        ui.text(i18n:T("settings.import.history.description"), MUTED),
    }
    if message ~= nil then
        table.insert(rows, ui.text(message.text, MUTED, {
            key = "settings-import-status",
            role = if message.error then "alert" else "status",
            color = if message.error then "status-danger" else "text-muted",
        }))
    end
    table.insert(rows, ui.row({ w = "full", justify = "end", gap = 8, pt = 4 }, {
        ui.button({
            key = "settings-import-cancel",
            text = i18n:T("common.cancel"),
            size = "small",
            variant = "neutral",
            onPress = view:Handler(function()
                onClose()
            end),
        }),
        ui.button({
            key = "settings-import-start",
            text = i18n:T("signins.action"),
            size = "small",
            variant = "contrast",
            autofocus = true,
            onPress = view:Handler(function()
                onImport()
            end),
        }),
    }))
    return ui.col({ key = "settings-browser-import-dialog", w = "full", pad = 20, gap = 12 }, rows)
end

return settings
