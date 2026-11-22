--!strict
-- The pane's offer to bring sign-ins, and with them the history, from the browser the person
-- already uses (official `import-prompt.tsx:16-265` `SignInOffer`), until they import or decline
-- once (`welcomed`). A `bar` sits above a page, where a signed-out site is the reason to import; a
-- `card` sits on the new tab page. Once an import brought something, the offer says so in its place
-- for 5 s or until closed. Nothing shows where no other browser is installed.

local ui = require("@opencode/ui")

local signins = {}

export type Choice = {
    sourceId: string,
    sourceName: string,
    profileId: string,
    label: string,
}

export type Message = {
    text: string,
    error: boolean,
}

export type Env = {
    I18n: I18nService,
    Menu: MenuService,
    choices: () -> { Choice },
    icon: (sourceId: string) -> ImageId?,
    blocked: () -> boolean,
    importing: (choice: Choice) -> boolean,
    message: () -> Message?,
    confirmed: () -> { choice: Choice?, text: string }?,
    run: (choice: Choice) -> (),
    -- Opens the import dialog for this profile with its sites listed (`import-prompt.tsx:90-93`).
    choose: (choice: Choice) -> (),
    decline: () -> (),
    more: () -> (),
    close: () -> (),
}

-- `choices.tsx` `profileChoices` / `choiceLabel`: one choice per profile, the browser used last
-- first; the profile's name only for a browser with several.
function signins.choices(sources: { any }): { Choice }
    local out: { Choice } = {}
    for _, source in sources do
        for _, profile in source.profiles do
            table.insert(out, {
                sourceId = source.id,
                sourceName = source.name,
                profileId = profile.id,
                label = if #source.profiles > 1 then `{source.name} · {profile.name}` else source.name,
            })
        end
    end
    return out
end

local BODY = table.freeze({ size = 13, weight = "regular", lineHeight = 16, color = "text-base", wrap = true })
local NAME = table.freeze({ size = 13, weight = "medium", lineHeight = 16, color = "text-base", truncate = true })

-- `choices.tsx` `Glyph`: the browser's own icon at 20 (16 in a menu), or the muted globe.
local function glyph(env: Env, sourceId: string?): ui.Node
    local image = if sourceId then env.icon(sourceId) else nil
    if image ~= nil then
        return ui.image({ image = image, w = 20, h = 20, fit = "contain", shrink = false })
    end
    return ui.row({ w = 20, h = 20, center = true, shrink = false }, {
        ui.icon("outline-globe", { size = 16, color = "icon-weak" }),
    })
end

local function messageText(message: Message, props: { [string]: any }): ui.Node
    local style: { [string]: any } = table.clone(BODY)
    for k, v in props do
        style[k] = v
    end
    style.role = if message.error then "alert" else "status"
    style.color = if message.error then "status-danger" else "text-base"
    return ui.text(message.text, style :: any)
end

-- The bar's frame (`border-b-[0.5px] border-v2-border-border-muted bg-layer-01 px-3 py-2`) or the
-- card's (`rounded-[8px] inset 0.5 border-muted`, inside a 608 column 48 from the top).
local function frame(variant: string, key: string, role: string?, label: string?, children: { ui.Node }): ui.Node
    local row = ui.row({
        key = key,
        role = role :: any,
        label = label,
        w = "full",
        gap = 12,
        px = 12,
        py = 8,
        align = "center",
        fill = "surface-1",
        radius = if variant == "card" then 8 else nil,
        border = if variant == "card" then "border-weak" else nil,
        borderWidth = if variant == "card" then 0.5 else nil,
    }, children)
    if variant == "card" then
        return ui.col({ w = "full", maxW = 608, px = 24, pt = 48 }, { row })
    end
    return ui.col({ w = "full", shrink = false }, { row, ui.divider({ color = "border-weak" } :: any) })
end

local function closeButton(env: Env, view: View, key: string, label: string, press: () -> ()): ui.Node
    return ui.iconButton({
        key = key,
        icon = "close",
        variant = "ghost-muted",
        size = "large",
        label = label,
        help = label,
        helpSide = "top",
        onPress = view:Handler(function()
            press()
        end),
    })
end

-- `import-prompt.tsx:103-130`: what the import brought, with the browser's icon and a check.
local function confirmation(env: Env, view: View, variant: string, done: { choice: Choice?, text: string }): ui.Node
    return frame(variant, "browser-signins", "status", nil, {
        glyph(env, if done.choice then done.choice.sourceId else nil),
        ui.icon("check", { size = 16, color = "status-success" }),
        ui.text(done.text, BODY, { grow = 1, shrink = true, minW = 0 }),
        closeButton(env, view, "browser-signins-close", env.I18n:T("common.close"), env.close),
    })
end

-- `import-prompt.tsx:186-261`: the browser used last, Import, the other browsers, Choose sites to
-- import and more in a menu, and Not now.
local function bar(env: Env, view: View, choices: { Choice }): ui.Node
    local first = choices[1]
    local message = env.message()
    local blocked = env.blocked()
    local menuKey = "browser-signins-more"
    local words = if message ~= nil
        then messageText(message, { grow = 1, shrink = true, minW = 0 })
        else ui.text(env.I18n:T("signins.bar", { browser = first.label }), BODY, { grow = 1, shrink = true, minW = 0 })
    return frame("bar", "browser-signins", nil, env.I18n:T("signins.title"), {
        glyph(env, first.sourceId),
        words,
        ui.row({ gap = 4, align = "center", shrink = false }, {
            ui.button({
                key = "browser-signins-import",
                text = env.I18n:T("signins.action"),
                size = "small",
                variant = "contrast",
                disabled = blocked,
                loading = env.importing(first),
                onPress = view:Handler(function()
                    env.run(first)
                end),
            }),
            ui.iconButton({
                key = menuKey,
                icon = "outline-chevron-down",
                variant = "ghost-muted",
                size = "large",
                label = env.I18n:T("signins.other"),
                help = env.I18n:T("signins.other"),
                helpSide = "top",
                menuTrigger = true,
                onPress = view:Handler(function()
                    local items: { MenuEntry } = {}
                    for i = 2, #choices do
                        table.insert(items, { id = `choice-{i}`, title = choices[i].label, enabled = not blocked })
                    end
                    if #choices > 1 then
                        table.insert(items, { kind = "separator" })
                    end
                    table.insert(items, { id = "choose", title = env.I18n:T("signins.choose"), enabled = not blocked })
                    table.insert(items, { id = "more", title = env.I18n:T("signins.more") })
                    local picked = env.Menu:OpenAsync({ anchorKey = menuKey, items = items, label = env.I18n:T("signins.other"), align = "end", width = 220 })
                    if picked == "choose" then
                        env.choose(first)
                    elseif picked == "more" then
                        env.more()
                    elseif picked ~= nil then
                        local index = tonumber(string.match(picked, "^choice%-(%d+)$"))
                        local choice = if index then choices[index] else nil
                        if choice then
                            env.run(choice)
                        end
                    end
                end),
            }),
            closeButton(env, view, "browser-signins-dismiss", env.I18n:T("signins.dismiss"), env.decline),
        }),
    })
end

-- `import-prompt.tsx:138-184`: every browser profile as a row with its own Import, then Choose
-- sites to import (for the browser used last), Not now and More import options.
local function card(env: Env, view: View, choices: { Choice }): ui.Node
    local blocked = env.blocked()
    local rows: { ui.Node } = {}
    for i, choice in choices do
        table.insert(rows, ui.row({ key = `browser-signins-choice-{i}`, w = "full", h = 44, gap = 12, align = "center" }, {
            glyph(env, choice.sourceId),
            ui.text(choice.label, NAME, { grow = 1, shrink = true, minW = 0 }),
            ui.button({
                key = `browser-signins-choice-btn-{i}`,
                text = env.I18n:T("signins.action"),
                size = "small",
                variant = "neutral",
                disabled = blocked,
                loading = env.importing(choice),
                onPress = view:Handler(function()
                    env.run(choice)
                end),
            }),
        }))
        if i < #choices then
            table.insert(rows, ui.divider({ color = "border-base" } :: any))
        end
    end
    local items: { ui.Node } = {
        ui.text(env.I18n:T("signins.title"), NAME),
        ui.col({ w = "full", px = 12, radius = 8, fill = "surface-1", border = "border-weak", borderWidth = 0.5 }, rows),
    }
    local message = env.message()
    if message ~= nil then
        table.insert(items, messageText(message, { lineHeight = 20 }))
    end
    table.insert(items, ui.row({ gap = 4, align = "center" }, {
        ui.button({
            key = "browser-signins-choose",
            text = env.I18n:T("signins.choose"),
            size = "small",
            variant = "ghost",
            disabled = blocked,
            onPress = view:Handler(function()
                env.choose(choices[1])
            end),
        }),
        ui.button({
            key = "browser-signins-decline",
            text = env.I18n:T("signins.dismiss"),
            size = "small",
            variant = "ghost",
            onPress = view:Handler(function()
                env.decline()
            end),
        }),
        ui.button({
            key = "browser-signins-settings",
            text = env.I18n:T("signins.more"),
            size = "small",
            variant = "ghost-muted",
            onPress = view:Handler(function()
                env.more()
            end),
        }),
    }))
    return ui.col({ key = "browser-signins", role = "region", label = env.I18n:T("signins.title"), w = "full", maxW = 608, px = 24, pt = 48, gap = 12 }, items)
end

-- The offer, its confirmation, or nothing (no other browser, or declined / already imported).
function signins.render(env: Env, view: View, variant: "bar" | "card", offering: boolean): ui.Node?
    local done = env.confirmed()
    if done ~= nil then
        return confirmation(env, view, variant, done)
    end
    if not offering then
        return nil
    end
    local choices = env.choices()
    if #choices == 0 then
        return nil
    end
    return if variant == "bar" then bar(env, view, choices) else card(env, view, choices)
end

return signins
