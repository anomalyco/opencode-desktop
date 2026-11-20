--!strict
-- Plain-language load and command failures matching upstream
-- `packages/gui-plugins/src/browser/failure.ts` and `failure-view.tsx`.

local ui = require("@opencode/ui")

local failure = {}

export type LoadKind = "unreachable" | "offline" | "slow" | "insecure" | "redirects" | "blocked" | "crashed" | "unavailable" | "generic"

export type LoadFailure = {
    kind: LoadKind,
    title: string,
    description: string,
    reload: boolean,
    code: string?,
}

export type Action = {
    key: string,
    label: string,
    run: () -> (),
}

export type Notice = {
    id: number,
    text: string,
    lasting: boolean,
    retry: (() -> ())?,
}

-- How long a transient command failure bar stays before dismissing itself (ms).
failure.NOTICE_MS = 8000

local LOAD_COPY: { [string]: { title: string, description: string } } = {
    unreachable = { title = "failed.unreachable.title", description = "failed.unreachable.description" },
    offline = { title = "failed.offline.title", description = "failed.offline.description" },
    slow = { title = "failed.slow.title", description = "failed.slow.description" },
    insecure = { title = "failed.insecure.title", description = "failed.insecure.description" },
    redirects = { title = "failed.redirects.title", description = "failed.redirects.description" },
    blocked = { title = "failed.blocked.title", description = "failed.blocked.description" },
    crashed = { title = "failed.crashed.title", description = "failed.crashed.description" },
    generic = { title = "failed.generic.title", description = "failed.generic.description" },
}

local function startsWith(s: string, prefix: string): boolean
    return string.sub(s, 1, #prefix) == prefix
end

local function classifyChromium(code: string): LoadKind
    if startsWith(code, "ERR_CERT_") or startsWith(code, "ERR_SSL_") or startsWith(code, "ERR_INSECURE_RESPONSE") or startsWith(code, "ERR_BAD_SSL_") then
        return "insecure"
    end
    if code == "ERR_TIMED_OUT" or code == "ERR_CONNECTION_TIMED_OUT" or code == "ERR_DNS_TIMED_OUT" then
        return "slow"
    end
    if code == "ERR_INTERNET_DISCONNECTED" or code == "ERR_NETWORK_CHANGED" or code == "ERR_NETWORK_ACCESS_DENIED" or code == "ERR_PROXY_CONNECTION_FAILED" then
        return "offline"
    end
    if code == "ERR_TOO_MANY_REDIRECTS" then
        return "redirects"
    end
    if startsWith(code, "ERR_BLOCKED_BY_") or startsWith(code, "ERR_UNSAFE_") or startsWith(code, "ERR_DISALLOWED_URL_SCHEME") or startsWith(code, "ERR_UNKNOWN_URL_SCHEME") or startsWith(code, "ERR_ACCESS_DENIED") then
        return "blocked"
    end
    if startsWith(code, "ERR_NAME_NOT_RESOLVED") or startsWith(code, "ERR_NAME_RESOLUTION_FAILED") or startsWith(code, "ERR_CONNECTION_") or startsWith(code, "ERR_ADDRESS_") or startsWith(code, "ERR_TUNNEL_CONNECTION_FAILED") or startsWith(code, "ERR_SOCKS_CONNECTION_") or startsWith(code, "ERR_INVALID_URL") then
        return "unreachable"
    end
    return "generic"
end

-- Maps a tab's `loadError` to plain-language copy, reload eligibility, and optional developer code
-- (`failure.ts::loadFailure`). HTTP status lines keep ONLY the 3-digit number, never site text.
function failure.load(errorText: string): LoadFailure
    if errorText == "browser.restore.outside" then
        return { kind = "unavailable", title = "failed.unavailable.title", description = "restore.outside", reload = false, code = nil }
    end
    if errorText == "browser.restore.refused" then
        return { kind = "unavailable", title = "failed.unavailable.title", description = "restore.refused", reload = false, code = nil }
    end
    if errorText == "browser.artifact.missing" then
        return { kind = "unavailable", title = "failed.unavailable.title", description = "artifact.missing", reload = true, code = nil }
    end
    if errorText == "page_crashed" then
        local c = LOAD_COPY.crashed
        return { kind = "crashed", title = c.title, description = c.description, reload = true, code = nil }
    end
    -- Bare 3-digit HTTP status only; never echo text supplied by a remote site.
    local status = string.match(errorText, "^(%d%d%d)$") or string.match(errorText, "^(%d%d%d)[^%d]")
    if status then
        local c = LOAD_COPY.generic
        return { kind = "generic", title = c.title, description = c.description, reload = true, code = status }
    end
    local code = string.match(errorText, "(ERR_[A-Z0-9_]+)")
    local kind: LoadKind = if code then classifyChromium(code) else "generic"
    local c = LOAD_COPY[kind] or LOAD_COPY.generic
    return { kind = kind, title = c.title, description = c.description, reload = true, code = code }
end

-- Plain-language sentence for an address refusal code (`failure.ts::refusalText`).
function failure.refusalKey(code: string): string
    if code == "browser.address.credentials" then
        return "refused.credentials"
    elseif code == "browser.address.workspace" then
        return "refused.workspace"
    elseif code == "browser.preview.restricted" then
        return "preview.restricted"
    end
    return "refused.web"
end

-- Renders the failed-load page where the document would sit (`failure-view.tsx::FailurePage`).
function failure.renderPage(view: View, i18n: I18nService, info: LoadFailure, actions: { Action }): ui.Node
    local iconName: IconName = if info.kind == "insecure" then "outline-hexagonal-warning" else "outline-globe"
    local children: { ui.Node } = {
        ui.icon(iconName, { size = 20, color = "icon-weak", mb = 8 }),
        ui.text(i18n:T(info.title), { size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", textAlign = "center" }),
        ui.text(i18n:T(info.description), { size = 13, weight = "regular", lineHeight = 20, color = "text-muted", textAlign = "center", wrap = true, maxW = 360 }),
    }
    if #actions > 0 then
        local buttons: { ui.Node } = {}
        for idx, act in actions do
            table.insert(buttons, ui.button({
                key = act.key,
                text = act.label,
                size = "small",
                variant = if idx == 1 then "neutral" else "ghost",
                onPress = view:Handler(function()
                    act.run()
                end),
            }))
        end
        table.insert(children, ui.row({ mt = 12, gap = 8, align = "center", justify = "center" }, buttons))
    end
    if info.code ~= nil and info.code ~= "" then
        table.insert(children, ui.text(info.code, {
            key = "browser-failed-code",
            dir = "ltr",
            mt = 16,
            size = 11,
            weight = "regular",
            lineHeight = 16,
            mono = true,
            color = "text-faint",
            textAlign = "center",
        }))
    end
    return ui.col({
        key = "browser-failed",
        dir = "auto",
        w = "full",
        grow = 1,
        minH = 0,
        px = 24,
        pt = 24,
        pb = 200,
        gap = 8,
        align = "center",
        justify = "center",
    }, children)
end

-- Renders a single-line notice bar above the page (`failure-view.tsx::FailureBar`).
function failure.renderBar(view: View, i18n: I18nService, key: string, words: string, retry: (() -> ())?, dismiss: () -> ()): ui.Node
    local rowItems: { ui.Node } = {
        ui.text(words, {
            role = "alert",
            size = 13,
            weight = "regular",
            lineHeight = 18,
            color = "text-chrome",
            wrap = true,
            grow = 1,
            shrink = true,
            minW = 0,
        }),
    }
    if retry then
        table.insert(rowItems, ui.button({
            key = `{key}-retry`,
            text = i18n:T("common.retry"),
            size = "small",
            variant = "neutral",
            onPress = view:Handler(function()
                retry()
            end),
        }))
    end
    table.insert(rowItems, ui.iconButton({
        key = `{key}-dismiss`,
        icon = "close",
        variant = "ghost-muted",
        size = "small",
        label = i18n:T("refused.dismiss"),
        help = i18n:T("refused.dismiss"),
        onPress = view:Handler(function()
            dismiss()
        end),
    }))
    return ui.col({ key = key, w = "full" }, {
        ui.row({ dir = "auto", w = "full", px = 12, py = 8, gap = 12, align = "center", fill = "surface-1" }, rowItems),
        ui.col({ w = "full", h = 1, fill = "border-weak", opacity = 0.5 }),
    })
end

return failure
