--!strict
-- Refused-navigation notices and 500 ms input activation guard matching upstream
-- `packages/gui-plugins/src/browser/activation-delay.ts`, `refusal-notice.ts`, and `refusal-bar.tsx`.

local ui = require("@opencode/ui")
local hostDisplay = require("./host_display")

local refusal = {}

-- How long a guarded control ignores presses after appearing or changing (`ACTIVATION_DELAY_MS`).
refusal.ACTIVATION_DELAY_MS = 500

export type Source = "conversation" | "page"

export type ExternalAction = "open" | "copy"

export type Notice = {
    token: string,
    origin: string,
    text: string,
    target: string?,
    action: ExternalAction?,
    shownAtMs: number,
}

-- Schemes permitted to offer Open from any source (`refusal-notice.ts` `apps`).
local APPS: { [string]: boolean } = {
    mailto = true,
    tel = true,
    sms = true,
    facetime = true,
    slack = true,
    msteams = true,
    zoommtg = true,
    zoomus = true,
    discord = true,
    spotify = true,
    figma = true,
    linear = true,
    notion = true,
    obsidian = true,
    tg = true,
    whatsapp = true,
    ["x-github-client"] = true,
    ["github-windows"] = true,
    ["github-mac"] = true,
}

-- Editor schemes may only offer Open from a user-activated page click, never from conversation.
local PAGE_ONLY: { [string]: boolean } = {
    vscode = true,
    ["vscode-insiders"] = true,
    cursor = true,
    zed = true,
    jetbrains = true,
}

-- Never offered as external app schemes (`refusal-notice.ts` `denied`).
local DENIED: { [string]: boolean } = {
    http = true,
    https = true,
    file = true,
    javascript = true,
    data = true,
    vbscript = true,
    chrome = true,
    ["chrome-extension"] = true,
    oc = true,
    ["oc-preview"] = true,
    workspace = true,
    about = true,
    blob = true,
    filesystem = true,
    shell = true,
    ["ms-msdt"] = true,
    ["search-ms"] = true,
    ["ms-officecmd"] = true,
}

-- Rejects control bytes, bidi overrides/isolates, and percent-encoded control bytes.
local function hasUnsafeChars(value: string): boolean
    for i = 1, #value do
        local b = string.byte(value, i)
        if b < 0x20 or b == 0x7f then
            return true
        end
    end
    -- C1 controls (U+0080..U+009F), bidi marks/overrides/isolates, BOM, and zero-width chars.
    local ok = pcall(function()
        for _, cp in utf8.codes(value) do
            if (cp >= 0x80 and cp <= 0x9f) or cp == 0x061c or (cp >= 0x200b and cp <= 0x200f) or (cp >= 0x202a and cp <= 0x202e) or cp == 0x2060 or (cp >= 0x2066 and cp <= 0x2069) or cp == 0xfeff then
                error("unsafe", 0)
            end
        end
    end)
    if not ok then
        return true
    end
    local lower = string.lower(value)
    if string.match(lower, "%%0[0-9a-f]") ~= nil or string.match(lower, "%%1[0-9a-f]") ~= nil or string.match(lower, "%%7f") ~= nil then
        return true
    end
    return false
end

-- Strict RFC 3986 URI character check: refuses `"`, spaces/whitespace, `\`, `< > ` ^ { | }`,
-- controls, non-ASCII, and malformed `%XX` / `%00`–`%1F` / `%7F` across EVERY scheme (`mailto:` included).
local function isStrictRfc3986Uri(value: string): boolean
    local len = #value
    if len < 1 or len > 2048 then
        return false
    end
    local i = 1
    while i <= len do
        local b = string.byte(value, i)
        if b == 0x25 then -- '%'
            if i + 2 > len then
                return false
            end
            local hex = string.sub(value, i + 1, i + 2)
            if string.match(hex, "^[0-9A-Fa-f][0-9A-Fa-f]$") == nil then
                return false
            end
            local dec = tonumber(hex, 16)
            if dec == nil or dec <= 0x1f or dec == 0x7f then
                return false
            end
            i += 3
        else
            local ok = (b >= 0x30 and b <= 0x39)
                or (b >= 0x41 and b <= 0x5a)
                or (b >= 0x61 and b <= 0x7a)
                or string.find("-._~:/?#[]@!$&'()*+,;=", string.char(b), 1, true) ~= nil
            if not ok then
                return false
            end
            i += 1
        end
    end
    return true
end

local function hasCredentials(value: string, schemeLen: number): boolean
    local rest = string.sub(value, schemeLen + 2)
    if string.sub(rest, 1, 2) ~= "//" then
        return false
    end
    local authority = string.match(string.sub(rest, 3), "^([^/?#]*)") or ""
    return string.find(authority, "@", 1, true) ~= nil
end

-- Strict RFC 6068 allow-list of `mailto:` query keys (`to`, `cc`, `bcc`, `subject`, `body`).
-- Preserves `+` verbatim (never turns `+` into space) and rejects any un-decodable key or value.
local MAILTO_ALLOWED_KEYS: { [string]: boolean } = {
    to = true,
    cc = true,
    bcc = true,
    subject = true,
    body = true,
}

local function percentDecodeStrict(raw: string): string?
    local out: { string } = {}
    local i = 1
    local len = #raw
    while i <= len do
        local ch = string.sub(raw, i, i)
        if ch == "%" then
            if i + 2 > len then
                return nil
            end
            local hex = string.sub(raw, i + 1, i + 2)
            if string.match(hex, "^[0-9A-Fa-f][0-9A-Fa-f]$") == nil then
                return nil
            end
            local byte = tonumber(hex, 16)
            if byte == nil or byte <= 0x1f or byte == 0x7f then
                return nil
            end
            table.insert(out, string.char(byte))
            i += 3
        else
            table.insert(out, ch)
            i += 1
        end
    end
    local decoded = table.concat(out)
    if hasUnsafeChars(decoded) then
        return nil
    end
    return decoded
end

local function mailtoQueryAllowed(value: string): boolean
    local tail = string.sub(value, 8)
    if tail == "" or string.sub(tail, 1, 2) == "//" or string.find(tail, "#", 1, true) ~= nil then
        return false
    end
    local pathPart = string.match(tail, "^([^?]*)") or ""
    if pathPart ~= "" and percentDecodeStrict(pathPart) == nil then
        return false
    end
    local query = string.match(tail, "%?(.*)$")
    if query == nil or query == "" then
        return true
    end
    for pair in string.gmatch(query, "[^&]+") do
        local rawKey, rawVal = string.match(pair, "^([^=]*)=(.*)$")
        if rawKey == nil or rawVal == nil then
            return false
        end
        local keyDec = percentDecodeStrict(rawKey)
        local valDec = percentDecodeStrict(rawVal)
        if keyDec == nil or valDec == nil then
            return false
        end
        if MAILTO_ALLOWED_KEYS[string.lower(keyDec)] ~= true then
            return false
        end
    end
    return true
end

-- Validates an external app URL (`refusal-notice.ts::externalTarget`), returning `"open"`,
-- `"copy"`, or `nil` when denied. Never trims away bad characters.
function refusal.externalTarget(value: string, source: Source?): ExternalAction?
    local src: Source = source or "page"
    if not isStrictRfc3986Uri(value) or hasUnsafeChars(value) then
        return nil
    end
    local rawScheme = string.match(value, "^([A-Za-z][A-Za-z0-9+.-]*):")
    if rawScheme == nil then
        return nil
    end
    local scheme = string.lower(rawScheme)
    if DENIED[scheme] or string.sub(scheme, 1, 8) == "opencode" then
        return nil
    end
    if hasCredentials(value, #rawScheme) then
        return nil
    end
    if scheme == "mailto" then
        return if mailtoQueryAllowed(value) then "open" else "copy"
    end
    if src == "conversation" and scheme == "obsidian" then
        local tail = string.sub(value, #rawScheme + 2)
        local okObsidian = string.match(tail, "^//open$") ~= nil or string.match(tail, "^//open%?") ~= nil or string.match(tail, "^//search$") ~= nil or string.match(tail, "^//search%?") ~= nil
        if not okObsidian then
            return "copy"
        end
    end
    local permitted = APPS[scheme] == true or (src == "page" and PAGE_ONLY[scheme] == true)
    return if permitted and #value <= 200 then "open" else "copy"
end

-- Returns whether `nowMs` is past the 500 ms activation delay and not held by an agent reveal guard.
function refusal.allowed(shownAtMs: number, nowMs: number, held: boolean): boolean
    return not held and nowMs >= shownAtMs + refusal.ACTIVATION_DELAY_MS
end

-- Renders the refusal notice bar (`refusal-bar.tsx::RefusalBar`). Buttons stay visually normal
-- during the 500 ms activation delay while ignoring early clicks so pages cannot trick users.
function refusal.renderBar(
    view: View,
    i18n: I18nService,
    notice: Notice,
    nowMs: () -> number,
    held: boolean,
    onAction: () -> (),
    onDismiss: () -> ()
): ui.Node
    local headerItems: { ui.Node } = {}
    if notice.origin ~= "" then
        table.insert(headerItems, ui.text(notice.origin, { size = 13, weight = "regular", lineHeight = 18, color = "text-chrome", wrap = true }))
    end
    table.insert(headerItems, ui.text(notice.text, { size = 13, weight = "regular", lineHeight = 18, color = "text-chrome", wrap = true }))

    local topItems: { ui.Node } = {
        ui.col({ role = "status", grow = 1, shrink = true, minW = 0, gap = 2 }, headerItems),
    }
    if notice.action ~= nil then
        local actionLabel = i18n:T(if notice.action == "open" then "refused.open" else "link.copy")
        table.insert(topItems, ui.button({
            key = "browser-refusal-action",
            text = actionLabel,
            size = "small",
            variant = "neutral",
            onPress = view:Handler(function()
                if refusal.allowed(notice.shownAtMs, nowMs(), held) then
                    onAction()
                end
            end),
        }))
    end
    table.insert(topItems, ui.iconButton({
        key = "browser-refusal-dismiss",
        icon = "close",
        variant = "ghost-muted",
        size = "small",
        label = i18n:T("refused.dismiss"),
        help = i18n:T("refused.dismiss"),
        onPress = view:Handler(function()
            if refusal.allowed(notice.shownAtMs, nowMs(), held) then
                onDismiss()
            end
        end),
    }))

    local rows: { ui.Node } = {
        ui.row({ w = "full", gap = 12, align = "center" }, topItems),
    }
    if notice.target ~= nil and notice.target ~= "" then
        local target = notice.target
        local web = hostDisplay.parts(target)
        local targetNode: ui.Node
        if web then
            targetNode = ui.row({ dir = "ltr", w = "full", minW = 0, align = "center" }, {
                ui.text(hostDisplay.displayHost(target), { size = 12, mono = true, lineHeight = 16, color = "text-muted", noWrap = true }),
                ui.text(web.rest, { size = 12, mono = true, lineHeight = 16, color = "text-muted", truncate = true, shrink = true, minW = 0 }),
            })
        else
            targetNode = ui.text(target, {
                dir = "ltr",
                size = 12,
                mono = true,
                lineHeight = 16,
                color = "text-muted",
                wrap = true,
            })
        end
        table.insert(rows, ui.col({ key = "browser-refusal-target", w = "full", maxH = 80, scrollY = true }, { targetNode }))
    end
    return ui.col({ key = "browser-refusal", w = "full" }, {
        ui.col({ dir = "auto", w = "full", px = 12, py = 8, gap = 4, fill = "surface-1" }, rows),
        ui.col({ w = "full", h = 1, fill = "border-weak", opacity = 0.5 }),
    })
end

return refusal
