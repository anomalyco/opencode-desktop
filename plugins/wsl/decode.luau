--!strict
-- A small JSON decoder, for settings earlier versions saved as JSON text. Nil for text that
-- isn't JSON.

local function decode(text: string): any
    local i = 1
    local n = #text

    local function space()
        while i <= n do
            local c = string.byte(text, i)
            if c ~= 32 and c ~= 9 and c ~= 10 and c ~= 13 then
                return
            end
            i += 1
        end
    end

    local value: () -> any

    local function str(): string
        i += 1
        local parts = {}
        while i <= n do
            local c = string.sub(text, i, i)
            if c == '"' then
                i += 1
                return table.concat(parts)
            elseif c == "\\" then
                local e = string.sub(text, i + 1, i + 1)
                if e == "u" then
                    local code = tonumber(string.sub(text, i + 2, i + 5), 16) or 63
                    table.insert(parts, utf8.char(code))
                    i += 6
                else
                    local map = { n = "\n", t = "\t", r = "\r", b = "\b", f = "\f" }
                    table.insert(parts, map[e] or e)
                    i += 2
                end
            else
                table.insert(parts, c)
                i += 1
            end
        end
        error("unterminated string")
    end

    value = function(): any
        space()
        local c = string.sub(text, i, i)
        if c == "{" then
            i += 1
            local out = {}
            space()
            if string.sub(text, i, i) == "}" then
                i += 1
                return out
            end
            while true do
                space()
                local key = str()
                space()
                i += 1 -- the colon
                out[key] = value()
                space()
                local sep = string.sub(text, i, i)
                i += 1
                if sep == "}" then
                    return out
                end
            end
        elseif c == "[" then
            i += 1
            local out = {}
            space()
            if string.sub(text, i, i) == "]" then
                i += 1
                return out
            end
            while true do
                table.insert(out, value())
                space()
                local sep = string.sub(text, i, i)
                i += 1
                if sep == "]" then
                    return out
                end
            end
        elseif c == '"' then
            return str()
        elseif string.sub(text, i, i + 3) == "true" then
            i += 4
            return true
        elseif string.sub(text, i, i + 4) == "false" then
            i += 5
            return false
        elseif string.sub(text, i, i + 3) == "null" then
            i += 4
            return nil
        end
        local num = string.match(text, "^-?%d+%.?%d*[eE]?[-+]?%d*", i)
        if not num then
            error("not JSON")
        end
        i += #num
        return tonumber(num)
    end

    local ok, result = pcall(value)
    return if ok then result else nil
end

return decode
