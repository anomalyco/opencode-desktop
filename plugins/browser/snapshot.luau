--!strict
-- `browser_snapshot`'s text: the page's accessibility tree at any depth within a character
-- budget, as Desktop's (`packages/gui-plugins/src/browser/snapshot.ts`): `@eN` refs on the
-- controls, `interactive` for the controls only, a line cap, a footer that says what to do when
-- it's cut, and each filled field's value unless it may be secret (`field-value.ts`), which
-- prints `value=<hidden>`.

local snapshot = {}

-- `snapshot.ts` `controls`.
local CONTROLS: { [string]: boolean } = {}
for _, role in { "button", "link", "textbox", "searchbox", "combobox", "listbox", "checkbox", "radio", "option", "switch", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "slider", "spinbutton", "treeitem", "scrollbar" } do
    CONTROLS[role] = true
end

local DEFAULT_CHARS = 20_000
-- A field's value prints at most this long (`field-value.ts`).
local VALUE_MAX = 160
-- Fields whose page details are read for one snapshot; any more print `value=<hidden>`.
local FIELDS_MAX = 50

-- What `describeField` (`field-value.ts`) reads from the element it runs on.
local DESCRIBE = [[function() {
  const parent = this.parentElement
  const own = parent && parent.querySelectorAll("input, textarea, [contenteditable]").length === 1
  return {
    type: this instanceof HTMLInputElement ? this.type : "",
    autocomplete: this.getAttribute("autocomplete") ?? "",
    labels: [this.id, this.getAttribute("name"), this.getAttribute("placeholder"), this.getAttribute("aria-label"), ...Array.from(this.labels ?? [], (label) => label.textContent)].filter(Boolean),
    masked: getComputedStyle(this).webkitTextSecurity !== "none",
    maxLength: this instanceof HTMLInputElement ? this.maxLength : -1,
    toggles: own ? Array.from(parent.querySelectorAll('button, [role="button"]'), (button) => button.getAttribute("aria-label") || button.textContent).filter(Boolean) : [],
  }
}]]

local SECRET_TOKENS: { [string]: boolean } = { ["current-password"] = true, ["new-password"] = true, ["one-time-code"] = true }
local SECRET_WORDS: { [string]: boolean } = { token = true, pwd = true, otp = true, totp = true, ["2fa"] = true, mfa = true, pin = true, cvv = true, cvc = true, ssn = true }
local KEY_KINDS: { [string]: boolean } = { api = true, private = true, access = true, signing = true, auth = true, ssh = true, webhook = true, encryption = true }
local CODE_KINDS: { [string]: boolean } = { security = true, verification = true, recovery = true, backup = true }
local CARD_PARTS: { [string]: boolean } = { number = true, num = true, no = true }

-- `field-value.ts` `secretWords` on a label made words: camel case split, non-word runs spaces.
local function secretLabel(label: string): boolean
    local words = string.gsub(label, "(%u+)(%u%l)", "%1 %2")
    words = string.gsub(words, "(%l)(%u)", "%1 %2")
    words = string.lower((string.gsub(words, "[%W_]+", " ")))
    if string.find(words, "passw", 1, true) or string.find(words, "passcode", 1, true) or string.find(words, "secret", 1, true) then
        return true
    end
    local list: { string } = {}
    for w in string.gmatch(words, "%S+") do
        table.insert(list, w)
    end
    for n, w in list do
        local nextWord = list[n + 1]
        if SECRET_WORDS[w] then
            return true
        end
        for kind in KEY_KINDS do
            if w == `{kind}key` or (w == kind and nextWord == "key") then
                return true
            end
        end
        for kind in CODE_KINDS do
            if w == `{kind}code` or (w == kind and nextWord == "code") then
                return true
            end
        end
        if string.sub(w, -4) == "card" and nextWord and CARD_PARTS[nextWord] then
            return true
        end
        for part in CARD_PARTS do
            local tail = `card{part}`
            if string.sub(w, -#tail) == tail then
                return true
            end
        end
    end
    return false
end

-- `field-value.ts` `readableField`: whether a snapshot may print the field's value. When unsure, it hides.
local function readable(field: { [string]: any }, name: string): boolean
    local kind = if type(field.type) == "string" then field.type else ""
    if field.masked == true or kind == "password" or kind == "file" then
        return false
    end
    if field.maxLength == 1 and (kind == "text" or kind == "tel" or kind == "number") then
        return false
    end
    for token in string.gmatch(string.lower(if type(field.autocomplete) == "string" then field.autocomplete else ""), "%S+") do
        if SECRET_TOKENS[token] or string.sub(token, 1, 3) == "cc-" then
            return false
        end
    end
    local labels: { string } = { name }
    for _, list in { field.labels, field.toggles } do
        if type(list) == "table" then
            for _, label in list do
                if type(label) == "string" then
                    table.insert(labels, label)
                end
            end
        end
    end
    for _, label in labels do
        if secretLabel(label) then
            return false
        end
    end
    return true
end

-- Whether the field `node` names may show its value, from what the page says about it.
local function fieldReadable(tab: BrowserTab, node: number, name: string): boolean
    local ok, out = pcall(function()
        local resolved = tab:DevtoolsAsync("DOM.resolveNode", { backendNodeId = node })
        local objectId = if type(resolved) == "table" and type(resolved.object) == "table" then resolved.object.objectId else nil
        if type(objectId) ~= "string" then
            return false
        end
        local described = tab:DevtoolsAsync("Runtime.callFunctionOn", { objectId = objectId, functionDeclaration = DESCRIBE, returnByValue = true })
        pcall(tab.DevtoolsAsync, tab, "Runtime.releaseObject", { objectId = objectId })
        local field = if type(described) == "table" and type(described.result) == "table" then described.result.value else nil
        return type(field) == "table" and readable(field, name)
    end)
    return ok and out == true
end

local function quote(s: string): string
    local escaped = string.gsub(s, '[\\"\n\r\t]', { ["\\"] = "\\\\", ['"'] = '\\"', ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" })
    return `"{escaped}"`
end

local function excerpt(s: string, limit: number): string
    if utf8.len(s) == nil or (utf8.len(s) :: number) <= limit then
        return s
    end
    local cut = utf8.offset(s, limit + 1) or (#s + 1)
    return `{string.sub(s, 1, cut - 1)}… [+{(utf8.len(s) :: number) - limit} chars]`
end

local function prop(node: { [string]: any }, name: string): any
    if type(node.properties) ~= "table" then
        return nil
    end
    for _, p in node.properties do
        if type(p) == "table" and p.name == name and type(p.value) == "table" then
            return p.value.value
        end
    end
    return nil
end

export type Options = { depth: number?, interactive: boolean, maxLines: number?, maxChars: number?, ref: (node: number) -> string }

-- The page's tree as lines, `@eN` refs from `options.ref`, within the budget, with a footer when cut.
function snapshot.render(tab: BrowserTab, tree: any, options: Options): string
    local nodes = if type(tree) == "table" and type(tree.nodes) == "table" then tree.nodes else {}
    local byId: { [string]: any } = {}
    for _, node in nodes do
        if type(node) == "table" and node.nodeId ~= nil then
            byId[tostring(node.nodeId)] = node
        end
    end
    local budget = math.clamp(math.floor(options.maxChars or DEFAULT_CHARS), 1_000, 200_000)
    local maxLines = if options.maxLines then math.max(1, math.floor(options.maxLines)) else math.huge
    local lines: { string } = {}
    local used = 0
    local total = 0
    local cut = false
    local fields = 0
    -- Depth first in document order from the root.
    local pending: { { node: any, level: number } } = {}
    if nodes[1] then
        table.insert(pending, { node = nodes[1], level = 0 })
    end
    while #pending > 0 do
        local item = table.remove(pending) :: { node: any, level: number }
        local node = item.node
        local children = if type(node.childIds) == "table" then node.childIds else {}
        if options.depth == nil or item.level < options.depth then
            for n = #children, 1, -1 do
                local child = byId[tostring(children[n])]
                if child then
                    table.insert(pending, { node = child, level = item.level + 1 })
                end
            end
        end
        if node.ignored ~= true then
            local role = if type(node.role) == "table" and type(node.role.value) == "string" then node.role.value else "node"
            local name = if type(node.name) == "table" and type(node.name.value) == "string" then string.gsub(node.name.value, "^%s*(.-)%s*$", "%1") else ""
            local id = node.backendDOMNodeId
            local editable = role == "textbox" or role == "searchbox" or prop(node, "editable") ~= nil
            local control = type(id) == "number" and role ~= "RootWebArea" and (CONTROLS[role] == true or editable)
            local listed = control or (not options.interactive and name ~= "" and role ~= "InlineTextBox")
            if listed then
                total += 1
            end
            if listed and not cut then
                local line = `{if control then `{options.ref(id)} ` else ""}[{role}] {quote(excerpt(name, 200))}`
                local value = if type(node.value) == "table" and type(node.value.value) == "string" then node.value.value else ""
                if editable and value ~= "" then
                    fields += 1
                    local shown = fields <= FIELDS_MAX and fieldReadable(tab, id :: number, name)
                    line ..= if shown then ` value={quote(excerpt(value, VALUE_MAX))}` else " value=<hidden>"
                end
                if used + #line + 1 > budget or #lines >= maxLines then
                    cut = true
                else
                    table.insert(lines, line)
                    used += #line + 1
                end
            end
        end
    end
    if cut then
        table.insert(lines, `[snapshot cut at {#lines} of {total} lines: pass interactive=true for controls only, or a larger maxChars or maxLines]`)
    end
    return table.concat(lines, "\n")
end


return snapshot
