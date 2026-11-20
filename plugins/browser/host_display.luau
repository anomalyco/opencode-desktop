--!strict
-- Canonical ASCII/punycode host display and left-elision matching upstream
-- `packages/gui-plugins/src/browser/host-display.ts`: keeps the registrable right edge visible
-- behind a leading ellipsis rather than cutting off the end, and converts non-ASCII IDN labels
-- to RFC 3492 punycode (`xn--...`) so homograph domains are shown safely.

local hostDisplay = {}

-- Maximum host characters shown as text in prompts, bars, and lists (`HOST_MAX`).
hostDisplay.HOST_MAX = 64

export type Parts = { scheme: string, host: string, rest: string }

local BASE = 36
local TMIN = 1
local TMAX = 26
local SKEW = 38
local DAMP = 700
local INITIAL_BIAS = 72
local INITIAL_N = 128

local function encodeDigit(d: number): string
    if d < 26 then
        return string.char(97 + d)
    end
    return string.char(48 + (d - 26))
end

local function adapt(delta: number, numPoints: number, first: boolean): number
    local d = if first then math.floor(delta / DAMP) else math.floor(delta / 2)
    d += math.floor(d / numPoints)
    local k = 0
    while d > math.floor(((BASE - TMIN) * TMAX) / 2) do
        d = math.floor(d / (BASE - TMIN))
        k += BASE
    end
    return k + math.floor(((BASE - TMIN + 1) * d) / (d + SKEW))
end

-- Decodes UTF-8 string into Unicode code points, returning nil on invalid UTF-8.
local function codepoints(s: string): { number }?
    local out: { number } = {}
    local ok = pcall(function()
        for _, cp in utf8.codes(s) do
            table.insert(out, cp)
        end
    end)
    return if ok then out else nil
end

-- RFC 3492 Bootstring / Punycode encoder for one non-ASCII domain label.
local function encodePunycode(label: string): string?
    local cps = codepoints(label)
    if cps == nil then
        return nil
    end
    local out: { string } = {}
    for _, cp in cps do
        if cp < 128 then
            local c = if cp >= 65 and cp <= 90 then cp + 32 else cp
            table.insert(out, string.char(c))
        end
    end
    local basic = #out
    local handled = basic
    if basic > 0 then
        table.insert(out, "-")
    end
    local n = INITIAL_N
    local delta = 0
    local bias = INITIAL_BIAS
    while handled < #cps do
        local m: number? = nil
        for _, cp in cps do
            if cp >= n and (m == nil or cp < m) then
                m = cp
            end
        end
        if m == nil then
            return nil
        end
        delta += (m - n) * (handled + 1)
        n = m
        for _, cp in cps do
            if cp < n then
                delta += 1
            elseif cp == n then
                local q = delta
                local k = BASE
                while true do
                    local t = if k <= bias then TMIN elseif k >= bias + TMAX then TMAX else k - bias
                    if q < t then
                        break
                    end
                    table.insert(out, encodeDigit(t + ((q - t) % (BASE - t))))
                    q = math.floor((q - t) / (BASE - t))
                    k += BASE
                end
                table.insert(out, encodeDigit(q))
                bias = adapt(delta, handled + 1, handled == basic)
                delta = 0
                handled += 1
            end
        end
        delta += 1
        n += 1
    end
    return table.concat(out)
end

local function isAscii(s: string): boolean
    for i = 1, #s do
        if string.byte(s, i) >= 128 then
            return false
        end
    end
    return true
end

-- Splits a host by ASCII dot or IDNA label separators (`\u{3002}`, `\u{FF0E}`, `\u{FF61}`).
local function splitLabels(raw: string): { string }
    local norm = string.gsub(raw, "\227\128\130", ".")
    norm = string.gsub(norm, "\239\188\142", ".")
    norm = string.gsub(norm, "\239\189\161", ".")
    local out: { string } = {}
    local start = 1
    while true do
        local dot = string.find(norm, ".", start, true)
        if dot == nil then
            table.insert(out, string.sub(norm, start))
            break
        end
        table.insert(out, string.sub(norm, start, dot - 1))
        start = dot + 1
    end
    return out
end

local function canonicalAuthority(authority: string): string?
    if authority == "" or string.find(authority, "@", 1, true) ~= nil then
        return nil
    end
    if string.find(authority, "[%c%s]") ~= nil then
        return nil
    end
    local rawHost = authority
    local port: string? = nil
    if string.sub(authority, 1, 1) == "[" then
        local close = string.find(authority, "]", 1, true)
        if close == nil then
            return nil
        end
        rawHost = string.sub(authority, 1, close)
        local tail = string.sub(authority, close + 1)
        if tail ~= "" then
            local p = string.match(tail, "^:(%d+)$")
            if p == nil then
                return nil
            end
            port = p
        end
        local lower = string.lower(rawHost)
        return if port then `{lower}:{port}` else lower
    else
        local h, p = string.match(authority, "^(.*):(%d+)$")
        if h ~= nil and p ~= nil then
            rawHost = h
            port = p
        end
    end
    if rawHost == "" then
        return nil
    end
    local labels = splitLabels(rawHost)
    local out: { string } = {}
    for _, label in labels do
        if isAscii(label) then
            table.insert(out, string.lower(label))
        else
            local enc = encodePunycode(label)
            if enc == nil then
                return nil
            end
            table.insert(out, `xn--{enc}`)
        end
    end
    local host = table.concat(out, ".")
    return if port then `{host}:{port}` else host
end

-- Left-elides `host` behind `"…"` when longer than `max` characters, preserving the registrable
-- right edge (domain + port).
function hostDisplay.elideLeft(host: string, max: number): string
    if max <= 0 or #host <= max then
        return host
    end
    local keep = math.max(1, max - 1)
    return `…{string.sub(host, #host - keep + 1)}`
end

-- The address bar's parts of a credential-free `http:` / `https:` URL (`hostDisplay` in upstream).
function hostDisplay.parts(address: string): Parts?
    local trimmed = string.gsub(address, "^%s*(.-)%s*$", "%1")
    local rawScheme = string.match(trimmed, "^([Hh][Tt][Tt][Pp][Ss]?://)")
    if rawScheme == nil then
        return nil
    end
    local scheme = string.lower(rawScheme)
    local after = string.sub(trimmed, #rawScheme + 1)
    local authority = string.match(after, "^([^/?#]*)") or ""
    local rest = string.sub(after, #authority + 1)
    local host = canonicalAuthority(authority)
    if host == nil then
        return nil
    end
    return {
        scheme = scheme,
        host = host,
        rest = if rest == "" then "/" else rest,
    }
end

-- The canonical ASCII authority of `url`, left-elided at `max` (default `HOST_MAX = 64`).
function hostDisplay.displayHost(url: string, max: number?): string
    local parts = hostDisplay.parts(url)
    if parts == nil then
        return ""
    end
    return hostDisplay.elideLeft(parts.host, max or hostDisplay.HOST_MAX)
end

return hostDisplay
