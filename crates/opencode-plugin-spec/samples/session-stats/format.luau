--!strict
-- Number formatting shared by the panel.

local format = {}

-- 12345 reads "12,345".
function format.int(n: number): string
    local digits = tostring(math.floor(n))
    local grouped = digits:reverse():gsub("(%d%d%d)", "%1,"):reverse()
    return (grouped:gsub("^,", ""))
end

-- 0.4 reads "$0.40".
function format.usd(n: number): string
    return string.format("$%.2f", n)
end

return format
