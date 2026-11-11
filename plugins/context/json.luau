--!strict
-- JSON as the server wrote it: an object's member and an array's items as their own text, and
-- `JSON.stringify(value, null, 2)`'s layout, both keeping the server's key order (a decoded table
-- would lose it).

local json = {}

local QUOTE, BACKSLASH = string.byte('"'), string.byte("\\")
local OPEN_OBJECT, CLOSE_OBJECT = string.byte("{"), string.byte("}")
local OPEN_ARRAY, CLOSE_ARRAY = string.byte("["), string.byte("]")
local COMMA, COLON = string.byte(","), string.byte(":")

local function isSpace(b: number?): boolean
    return b == 32 or b == 9 or b == 10 or b == 13
end

local function skipSpace(s: string, i: number): number
    while isSpace(string.byte(s, i)) do
        i += 1
    end
    return i
end

-- The index after the string that opens at `i`.
local function stringEnd(s: string, i: number): number
    i += 1
    local n = #s
    while i <= n do
        local b = string.byte(s, i)
        if b == BACKSLASH then
            i += 2
        elseif b == QUOTE then
            return i + 1
        else
            i += 1
        end
    end
    return i
end

-- The index after the value that starts at `i` (after any space).
local function valueEnd(s: string, i: number): number
    i = skipSpace(s, i)
    local b = string.byte(s, i)
    if b == QUOTE then
        return stringEnd(s, i)
    end
    local n = #s
    if b == OPEN_OBJECT or b == OPEN_ARRAY then
        local depth = 0
        while i <= n do
            local c = string.byte(s, i)
            if c == QUOTE then
                i = stringEnd(s, i)
                continue
            elseif c == OPEN_OBJECT or c == OPEN_ARRAY then
                depth += 1
            elseif c == CLOSE_OBJECT or c == CLOSE_ARRAY then
                depth -= 1
                if depth == 0 then
                    return i + 1
                end
            end
            i += 1
        end
        return i
    end
    while i <= n do
        local c = string.byte(s, i)
        if c == COMMA or c == CLOSE_OBJECT or c == CLOSE_ARRAY or isSpace(c) then
            break
        end
        i += 1
    end
    return i
end

-- The text of member `key` of the object `obj`.
function json.member(obj: string, key: string): string?
    local i = skipSpace(obj, 1)
    if string.byte(obj, i) ~= OPEN_OBJECT then
        return nil
    end
    i += 1
    while true do
        i = skipSpace(obj, i)
        if string.byte(obj, i) ~= QUOTE then
            return nil
        end
        local stop = stringEnd(obj, i)
        local name = string.sub(obj, i + 1, stop - 2)
        i = skipSpace(obj, stop)
        if string.byte(obj, i) ~= COLON then
            return nil
        end
        local start = skipSpace(obj, i + 1)
        local finish = valueEnd(obj, start)
        if name == key then
            return string.sub(obj, start, finish - 1)
        end
        i = skipSpace(obj, finish)
        if string.byte(obj, i) ~= COMMA then
            return nil
        end
        i += 1
    end
end

-- The text of each item of the array `arr`.
function json.items(arr: string): { string }
    local out: { string } = {}
    local i = skipSpace(arr, 1)
    if string.byte(arr, i) ~= OPEN_ARRAY then
        return out
    end
    i = skipSpace(arr, i + 1)
    if string.byte(arr, i) == CLOSE_ARRAY then
        return out
    end
    local n = #arr
    while i <= n do
        local start = skipSpace(arr, i)
        local finish = valueEnd(arr, start)
        table.insert(out, string.sub(arr, start, finish - 1))
        i = skipSpace(arr, finish)
        if string.byte(arr, i) ~= COMMA then
            break
        end
        i += 1
    end
    return out
end

-- `JSON.stringify(JSON.parse(raw), null, 2)`'s layout, in the order `raw` has.
function json.pretty(raw: string): string
    local out: { string } = {}
    local depth = 0
    local i = 1
    local n = #raw
    local function newline()
        table.insert(out, "\n" .. string.rep("  ", depth))
    end
    while i <= n do
        local c = string.byte(raw, i)
        if c == QUOTE then
            local stop = stringEnd(raw, i)
            table.insert(out, string.sub(raw, i, stop - 1))
            i = stop
            continue
        elseif c == OPEN_OBJECT or c == OPEN_ARRAY then
            local close = if c == OPEN_OBJECT then CLOSE_OBJECT else CLOSE_ARRAY
            local nextAt = skipSpace(raw, i + 1)
            if string.byte(raw, nextAt) == close then
                table.insert(out, string.char(c, close))
                i = nextAt + 1
                continue
            end
            table.insert(out, string.char(c))
            depth += 1
            newline()
        elseif c == CLOSE_OBJECT or c == CLOSE_ARRAY then
            depth = math.max(depth - 1, 0)
            newline()
            table.insert(out, string.char(c))
        elseif c == COMMA then
            table.insert(out, ",")
            newline()
        elseif c == COLON then
            table.insert(out, ": ")
        elseif not isSpace(c) then
            local stop = valueEnd(raw, i)
            table.insert(out, string.sub(raw, i, stop - 1))
            i = stop
            continue
        end
        i += 1
    end
    return table.concat(out)
end

local ESCAPES = table.freeze({ ['"'] = '"', ["\\"] = "\\", ["/"] = "/", b = "\b", f = "\f", n = "\n", r = "\r", t = "\t" })

-- The string a JSON string literal (`raw`, quotes included) holds; nil for anything else.
function json.string(raw: string?): string?
    if raw == nil or string.byte(raw, 1) ~= QUOTE then
        return nil
    end
    local body = string.sub(raw, 2, -2)
    if not string.find(body, "\\", 1, true) then
        return body
    end
    local out: { string } = {}
    local i, n = 1, #body
    while i <= n do
        local at = string.find(body, "\\", i, true)
        if at == nil then
            table.insert(out, string.sub(body, i))
            break
        end
        table.insert(out, string.sub(body, i, at - 1))
        local c = string.sub(body, at + 1, at + 1)
        if c == "u" then
            local code = tonumber(string.sub(body, at + 2, at + 5), 16) or 0xFFFD
            i = at + 6
            -- A surrogate pair is one character.
            if code >= 0xD800 and code <= 0xDBFF and string.sub(body, i, i + 1) == "\\u" then
                local low = tonumber(string.sub(body, i + 2, i + 5), 16) or 0
                if low >= 0xDC00 and low <= 0xDFFF then
                    code = 0x10000 + (code - 0xD800) * 0x400 + (low - 0xDC00)
                    i += 6
                end
            end
            table.insert(out, utf8.char(code))
        else
            table.insert(out, ESCAPES[c] or c)
            i = at + 2
        end
    end
    return table.concat(out)
end

return json
