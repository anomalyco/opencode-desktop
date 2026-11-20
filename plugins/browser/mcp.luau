--!strict
-- The browser's tools on the app's client MCP server (`browser_*`): the agent lists, opens,
-- navigates, shows and closes the session's pages, reads a page's accessibility tree with `@eN`
-- references, highlights one, runs JavaScript, waits for the page, answers its dialogs, takes a
-- screenshot and sends raw DevTools commands. Each answers for the session whose agent called it.
-- A page that shows a dialog refuses every command but `browser_dialog`, `browser_tabs` and
-- `browser_close` until it's answered, as Desktop's dialog gate does (`dialog-machine.ts`).

local snapshot = require("./snapshot")

local mcp = {}

local MAX_TEXT_BYTES = 32 * 1024
local UNSUPPORTED = "The browser isn't available on this platform yet in OpenCode (Rune)"

local function truncate(s: string): string
    if #s <= MAX_TEXT_BYTES then
        return s
    end
    -- Cut on a UTF-8 boundary.
    local cut = MAX_TEXT_BYTES
    while cut > 0 and bit32.band(string.byte(s, cut + 1) or 0, 0xC0) == 0x80 do
        cut -= 1
    end
    return `{string.sub(s, 1, cut)}… [truncated {#s - cut} bytes]`
end

-- A name as Rust's `{:?}` quotes it.
local function quote(s: string): string
    local escaped = string.gsub(s, '[\\"\n\r\t]', { ["\\"] = "\\\\", ['"'] = '\\"', ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" })
    return `"{escaped}"`
end

local function compact(v: any): any
    if type(v) == "string" then
        return truncate(v)
    end
    return v
end

local function tabSchema(description: string): { [string]: any }
    return { type = "integer", minimum = 1, description = description }
end

local DEFAULT_TAB = "Browser tab ID (defaults to this session's active browser tab)."

function mcp.add(ctx: WindowContext, Browser: BrowserService, Layout: LayoutService)
    -- Builds with no engine at all (Linux arm64, web, or a release build with no engine source or
    -- trust rule such as Linux x64 and Windows today) offer none of the browser tools.
    local initial = Browser:Engine().state
    if initial == "unsupported" or initial == "failed" then
        return
    end

    -- Each page's `@eN` references from its last snapshot, and the next number to hand out.
    local refs: { [string]: { [string]: number } } = {}
    local nextRef = 1

    local function id(tab: BrowserTab): number
        return tonumber(tab.Id) or 0
    end

    local function page(tab: BrowserTab): { [string]: any }
        return { tabID = id(tab), url = tab:Url(), title = tab:Title() }
    end

    -- The session's open tab ids, for an error that names the ones to use.
    local function openIds(sid: string): string
        local ids: { string } = {}
        for _, tab in Browser:Tabs(sid) do
            table.insert(ids, tostring(id(tab)))
        end
        return if #ids == 0 then "none" else table.concat(ids, ", ")
    end

    -- The page `args.tabID` names, else the session's: a missing one raises the tool's error.
    local function target(args: { [string]: any }, sid: string): BrowserTab
        local wanted = args.tabID
        local tab = if wanted ~= nil then Browser:Get(tostring(wanted)) else Browser:Active(sid)
        if tab == nil then
            error(if wanted ~= nil then `No browser tab with tabID {wanted}. Open tabs: {openIds(sid)}; list them with browser_tabs` else "No open browser tab: open one with browser_open", 0)
        end
        return tab
    end

    -- A page that waits on a dialog runs nothing else until it's answered (`chromium.ts` `runStopError`).
    local function ready(tab: BrowserTab): BrowserTab
        if tab:Dialog() ~= nil then
            error(`[browser.dialog_open] A JavaScript dialog is open. Inspect it with browser_dialog(\{"tabID":{id(tab)},"action":"get"\}), then explicitly accept or dismiss it before continuing.`, 0)
        end
        return tab
    end

    -- Sessions whose agent used these tools, and which of them run a turn: such a session's next
    -- turn may well open a page again, so the engine starts as the turn does when none of its
    -- pages is open (every turn would start it for the many that never open one).
    local used: { [string]: boolean } = {}
    local busy: { [string]: boolean } = {}
    local Sessions = ctx:GetService("Sessions")
    Sessions.Changed:Connect(function(session: Session)
        if not used[session.id] then
            return
        end
        local started = session.busy and not busy[session.id]
        busy[session.id] = if session.busy then true else nil
        if started and #Browser:Tabs(session.id) == 0 then
            Browser:Prepare(30)
        end
    end)

    local function checkEngine()
        if not Browser:Available() then
            error(UNSUPPORTED, 0)
        end
        local eng = Browser:Engine()
        if eng.state == "unsupported" then
            error(UNSUPPORTED, 0)
        end
        if eng.state == "failed" then
            error(eng.message or "Browser engine failed to start", 0)
        end
    end

    local function waitEngine(created: BrowserTab?)
        for _ = 1, 600 do
            if Browser:Engine().state ~= "starting" then
                break
            end
            task.wait(0.05)
        end
        local eng = Browser:Engine()
        if eng.state == "ready" then
            return
        end
        if created ~= nil then
            created:Close()
        end
        if eng.state == "unsupported" then
            error(UNSUPPORTED, 0)
        end
        if eng.state == "failed" then
            error(eng.message or "Browser engine failed to start", 0)
        end
        if eng.state == "starting" then
            error("Browser engine is still starting; try again in a moment", 0)
        end
        error("Browser engine isn't installed yet in OpenCode (Rune)", 0)
    end

    -- Answer once the page says where it went (it changed address and stopped loading, was refused
    -- after `beforeRejected`, or failed), as Desktop's navigate answers after the load; at most 10 s.
    local function waitLoad(tab: BrowserTab, before: string, beforeRejected: number)
        local loaded = tab:Loading()
        for _ = 1, 100 do
            -- The agent's navigation leaves without "Leave site?", as Desktop's does.
            local asked = tab:Dialog()
            if asked and asked.kind == "leave" then
                tab:AnswerDialog(asked.id, true)
            end
            if tab:Rejected() > beforeRejected or tab:LoadError() ~= nil or ((loaded or tab:Url() ~= before) and not tab:Loading()) then
                break
            end
            task.wait(0.1)
            loaded = loaded or tab:Loading()
        end
    end

    local function refusalMessage(code: string): string
        if code == "browser.address.credentials" then
            return "URLs with a user name or password are not supported. Remove them from the URL."
        end
        if code == "browser.address.workspace" then
            return "Only HTTP, HTTPS, about:blank, and file URLs inside the workspace are supported."
        end
        return "Only HTTP, HTTPS, and about:blank URLs are supported."
    end

    local function normalizeUrl(raw: string, sid: string): string
        local ok, out = pcall(Browser.NormalizeUrl, Browser, raw, sid)
        if not ok then
            local msg = string.gsub(tostring(out), "^runtime error: ", "")
            msg = string.match(msg, "^([^\r\n]+)") or msg
            error(msg, 0)
        end
        return out
    end

    local function tool(name: string, description: string, properties: { [string]: any }, required: { string }?, call: (args: { [string]: any }, sid: string) -> any)
        ctx:AddMcpTool({
            name = name,
            description = description,
            inputSchema = { type = "object", properties = properties, required = required, additionalProperties = false },
            call = function(args, context)
                used[context.sessionId] = true
                busy[context.sessionId] = true
                return call(args, context.sessionId)
            end,
        })
    end

    tool("browser_tabs", "List browser tabs in this session's OpenCode (Rune) side panel and which tab is focused.", {}, nil, function(_, sid)
        local tabs = {}
        for _, tab in Browser:Tabs(sid) do
            table.insert(tabs, { tabID = id(tab), url = tab:Url(), title = tab:Title(), loading = tab:Loading(), canGoBack = tab:CanGoBack(), canGoForward = tab:CanGoForward() })
        end
        local focused = Browser:Active(sid)
        return { open = Layout:RegionOpen(sid), focusedTabID = if focused then id(focused) else nil, tabs = tabs }
    end)

    tool("browser_open", "Open a URL or local ':port' in a new browser tab in the OpenCode (Rune) side panel.", {
        url = { type = "string", description = "URL or ':port' address to open (defaults to 'about:blank')." },
    }, nil, function(args, sid)
        checkEngine()
        local normalized = normalizeUrl(if type(args.url) == "string" then args.url else "about:blank", sid)
        local tab = Browser:OpenAsync({ url = normalized, sessionId = sid, background = true })
        if tab == nil then
            error(UNSUPPORTED, 0)
        end
        waitEngine(tab)
        if normalized ~= "about:blank" then
            waitLoad(tab, "", 0)
        end
        if tab:Rejected() > 0 then
            local refused = tab:Refusal() or "browser.address.web"
            tab:Close()
            error(refusalMessage(refused), 0)
        end
        return { tabID = id(tab), url = tab:Url() }
    end)

    tool("browser_navigate", "Navigate a browser tab in OpenCode (Rune) to a URL or ':port', opening a tab if none exists.", {
        url = { type = "string", description = "URL or ':port' address to navigate to." },
        tabID = tabSchema("Target browser tab ID (defaults to this session's active browser tab)."),
    }, { "url" }, function(args, sid)
        local url = if type(args.url) == "string" then args.url else ""
        if string.match(url, "^%s*$") then
            error("Missing 'url'", 0)
        end
        checkEngine()
        if args.tabID ~= nil and Browser:Get(tostring(args.tabID)) == nil then
            error(`No browser tab with tabID {args.tabID}`, 0)
        end
        local normalized = normalizeUrl(url, sid)
        local tab = if args.tabID ~= nil then Browser:Get(tostring(args.tabID)) else Browser:Active(sid)
        if tab == nil then
            local opened = Browser:OpenAsync({ url = normalized, sessionId = sid, background = true })
            if opened == nil then
                error(UNSUPPORTED, 0)
            end
            waitEngine(opened)
            if normalized ~= "about:blank" then
                waitLoad(opened, "", 0)
            end
            if opened:Rejected() > 0 then
                local refused = opened:Refusal() or "browser.address.web"
                opened:Close()
                error(refusalMessage(refused), 0)
            end
            return { tabID = id(opened), url = opened:Url() }
        end
        ready(tab)
        local before = tab:Url()
        local beforeRejected = tab:Rejected()
        tab:Navigate(normalized)
        tab:Show({ sessionId = sid, background = true })
        task.wait(0.05)
        waitEngine(nil)
        waitLoad(tab, before, beforeRejected)
        if tab:Rejected() > beforeRejected then
            error(refusalMessage(tab:Refusal() or "browser.address.web"), 0)
        end
        return { tabID = id(tab), url = tab:Url() }
    end)

    tool("browser_focus", "Show and focus an existing browser tab in the OpenCode (Rune) side panel.", {
        tabID = { type = "integer", minimum = 1, description = "Browser tab ID from browser_tabs." },
    }, { "tabID" }, function(args, sid)
        if args.tabID == nil then
            error("Missing 'tabID'", 0)
        end
        local tab = Browser:Get(tostring(args.tabID))
        if tab == nil then
            error(`No browser tab with tabID {args.tabID}`, 0)
        end
        tab:Show({ sessionId = sid, background = true })
        return page(tab)
    end)

    tool("browser_close", "Close a browser tab in the OpenCode (Rune) side panel.", {
        tabID = tabSchema("Browser tab ID to close (defaults to this session's active browser tab)."),
    }, nil, function(args, sid)
        local tab = target(args, sid)
        tab:Close()
        return { closedTabID = id(tab) }
    end)

    tool("browser_snapshot", "Read the accessibility tree of a browser tab in OpenCode (Rune) at any depth, with '@eN' refs on its controls and each filled field's value (a field that may be secret prints value=<hidden>). Long pages are cut at maxChars (default 20000) with a footer saying how to see more.", {
        tabID = tabSchema(DEFAULT_TAB),
        interactive = { type = "boolean", description = "List the controls only." },
        maxLines = { type = "integer", minimum = 1, description = "Print at most this many lines." },
        maxChars = { type = "integer", minimum = 1000, maximum = 200000, description = "Print at most this many characters (default 20000)." },
        depth = { type = "integer", minimum = 1, description = "Read at most this deep (default: the whole tree)." },
    }, nil, function(args, sid)
        local tab = ready(target(args, sid))
        local tree = tab:DevtoolsAsync("Accessibility.getFullAXTree", {})
        local mine: { [string]: number } = {}
        refs[tab.Id] = mine
        local out = page(tab)
        out.snapshot = snapshot.render(tab, tree, {
            depth = if type(args.depth) == "number" then math.max(1, math.floor(args.depth)) else nil,
            interactive = args.interactive == true,
            maxLines = if type(args.maxLines) == "number" then args.maxLines else nil,
            maxChars = if type(args.maxChars) == "number" then args.maxChars else nil,
            ref = function(node: number): string
                local ref = `@e{nextRef}`
                nextRef += 1
                mine[ref] = node
                return ref
            end,
        })
        return out
    end)

    tool("browser_highlight", "Highlight an '@eN' accessibility ref in a browser tab in OpenCode (Rune), or omit 'ref' to clear.", {
        ref = { type = "string", description = "Element ref from browser_snapshot (e.g. '@e3'); omit or pass '' to clear." },
        tabID = tabSchema(DEFAULT_TAB),
    }, nil, function(args, sid)
        local tab = ready(target(args, sid))
        local raw = if type(args.ref) == "string" then string.gsub(args.ref, "^%s*(.-)%s*$", "%1") else ""
        if raw == "" then
            tab:Highlight(nil)
            return { tabID = id(tab) }
        end
        local key = `@{string.gsub(raw, "^@+", "")}`
        local node = (refs[tab.Id] or {})[key]
        if node == nil then
            error(`Unknown ref '{key}' for tab {id(tab)}; run browser_snapshot first`, 0)
        end
        tab:Highlight(node)
        return { tabID = id(tab), ref = key, backendNodeId = node }
    end)

    -- The page runs a script as a classic script, where a top-level `return` or `await` doesn't
    -- compile. One that fails so (nothing of it ran) runs again in an async function: as its
    -- returned expression when it is one (`await fetch(x)`), else as its body (`tools.ts` `asyncBody`).
    local function evaluate(tab: BrowserTab, script: string): any
        local function run(source: string): (any, string?)
            local answer = tab:DevtoolsAsync("Runtime.evaluate", { expression = source, returnByValue = true, awaitPromise = true })
            if type(answer) ~= "table" then
                return {}, nil
            end
            local exc = answer.exceptionDetails
            if type(exc) == "table" then
                return nil, if type(exc.exception) == "table" and type(exc.exception.description) == "string" then exc.exception.description elseif type(exc.text) == "string" then exc.text else "Evaluation failed"
            end
            return answer, nil
        end
        local answer, failure = run(script)
        local syntax = failure ~= nil and string.find(failure, "SyntaxError", 1, true) ~= nil
        if syntax and (string.find(script, "%f[%w_]return%f[^%w_]") or string.find(script, "%f[%w_]await%f[^%w_]")) then
            local body = string.gsub(string.gsub(script, "^%s+", ""), "[%s;]+$", "")
            answer, failure = run(`(async () => (\n{body}\n))()`)
            if failure ~= nil and string.find(failure, "SyntaxError", 1, true) then
                answer, failure = run(`(async () => \{\n{script}\n\})()`)
            end
        end
        if failure ~= nil then
            error(failure, 0)
        end
        return answer
    end

    tool("browser_evaluate", "Evaluate JavaScript in a browser tab in OpenCode (Rune) and return its value: an expression, or statements with a top-level 'return' or 'await'. Promises are awaited. 'script' and 'code' are accepted for 'expression'.", {
        expression = { type = "string", description = "JavaScript to evaluate, e.g. 'document.title' or 'const r = await fetch(\"/api\"); return r.status'." },
        script = { type = "string", description = "Same as 'expression'." },
        code = { type = "string", description = "Same as 'expression'." },
        tabID = tabSchema(DEFAULT_TAB),
    }, nil, function(args, sid)
        local script = if type(args.expression) == "string" then args.expression elseif type(args.script) == "string" then args.script elseif type(args.code) == "string" then args.code else nil
        if script == nil then
            error("browser_evaluate needs the JavaScript to run in 'expression', for example browser_evaluate({ expression: \"document.title\" }).", 0)
        end
        local tab = ready(target(args, sid))
        local answer = evaluate(tab, script)
        local result = if type(answer.result) == "table" then answer.result else {}
        local value: any
        if result.value ~= nil then
            value = compact(result.value)
        elseif type(result.description) == "string" then
            value = truncate(result.description)
        else
            value = result
        end
        return { tabID = id(tab), value = value }
    end)

    -- Desktop's wait (`rpc.ts` "wait"): a timeout is a result (`met: false`), not an error.
    tool("browser_wait", "Wait until a condition holds in a browser tab in OpenCode (Rune), or pause. Pass one of: text (this text appears on the page), textGone (it's no longer there), selector (a matching element is visible), url (the address contains it), or nothing (the page finishes loading). With only timeout, it pauses that long. A timeout returns met: false with what the page showed. timeout is in milliseconds, default 10000, at most 30000.", {
        tabID = tabSchema(DEFAULT_TAB),
        text = { type = "string", description = "Wait until this literal text is on the page." },
        textGone = { type = "string", description = "Wait until this literal text is no longer on the page." },
        selector = { type = "string", description = "CSS selector: wait until a matching element is visible." },
        url = { type = "string", description = "Wait until the tab's address contains this." },
        timeout = { type = "integer", minimum = 0, description = "Milliseconds to wait (default 10000, at most 30000)." },
    }, nil, function(args, sid)
        local tab = ready(target(args, sid))
        local timeout = math.clamp(if type(args.timeout) == "number" then args.timeout else 10_000, 0, 30_000) / 1000
        local function str(v: any): string?
            return if type(v) == "string" and v ~= "" then v else nil
        end
        local text, gone, selector, url = str(args.text), str(args.textGone), str(args.selector), str(args.url)
        local waited = if text then "text" elseif gone then "textGone" elseif selector then "selector" elseif url then "url" elseif args.timeout ~= nil then "delay" else "load"
        local function holds(): boolean
            if waited == "delay" then
                return false
            elseif waited == "load" then
                return not tab:Loading()
            elseif waited == "url" then
                return string.find(tab:Url(), url :: string, 1, true) ~= nil
            end
            local check = if waited == "selector"
                then `(() => \{ const e = document.querySelector({quote(selector :: string)}); if (!e) return false; const r = e.getBoundingClientRect(); const s = getComputedStyle(e); return r.width > 0 && r.height > 0 && s.visibility !== "hidden" \})()`
                else `(document.body ? document.body.innerText : "").includes({quote((text or gone) :: string)})`
            local ok, answer = pcall(evaluate, tab, check)
            local found = ok and type(answer) == "table" and type(answer.result) == "table" and answer.result.value == true
            return if waited == "textGone" then ok and not found else found
        end
        local System = ctx:GetService("System")
        local deadline = System:Now() / 1000 + timeout
        local met = holds()
        while not met and System:Now() / 1000 < deadline do
            task.wait(0.1)
            if tab:Dialog() ~= nil then
                break
            end
            met = holds()
        end
        if waited == "delay" then
            met = true
        end
        return { tabID = id(tab), met = met, waited = waited, observed = { url = tab:Url(), title = tab:Title(), loading = tab:Loading(), dialog = tab:Dialog() ~= nil } }
    end)

    tool("browser_dialog", "Inspect, accept or dismiss the alert, confirm, prompt or 'Leave site?' a browser tab in OpenCode (Rune) waits on. The person may answer it in the pane too: the first answer wins. No dialog is reported as null.", {
        tabID = tabSchema(DEFAULT_TAB),
        action = { type = "string", enum = { "get", "accept", "dismiss" }, description = "'get' reads it; 'accept' presses OK (or Leave); 'dismiss' presses Cancel (or Stay)." },
        promptText = { type = "string", description = "The answer to a prompt, with 'accept'." },
    }, { "action" }, function(args, sid)
        local tab = target(args, sid)
        local dialog = tab:Dialog()
        local function shown(d: BrowserDialog?): any
            return if d then { type = d.kind, message = truncate(d.message), defaultValue = d.defaultText } else nil
        end
        local out = page(tab)
        if args.action == "get" or args.action == nil then
            out.dialog = shown(dialog)
            return out
        end
        if dialog == nil then
            error(`No dialog is open in tab {id(tab)}.`, 0)
        end
        local accept = args.action == "accept"
        local text = if accept and type(args.promptText) == "string" then args.promptText else dialog.defaultText
        if not tab:AnswerDialog(dialog.id, accept, text) then
            error(`The dialog in tab {id(tab)} was already answered; read the tab again with browser_dialog action 'get'.`, 0)
        end
        out.dialog = nil
        return out
    end)

    tool("browser_screenshot", "Capture a screenshot of a browser tab in OpenCode (Rune), returning compact metadata and the image.", {
        tabID = tabSchema(DEFAULT_TAB),
        format = { type = "string", enum = { "png", "jpeg", "webp" }, description = "Image format (default 'png')." },
    }, nil, function(args, sid)
        local format = if args.format == "jpeg" or args.format == "jpg" then "jpeg" elseif args.format == "webp" then "webp" else "png"
        local tab = ready(target(args, sid))
        local shot = tab:DevtoolsAsync("Page.captureScreenshot", { format = format })
        local data = if type(shot) == "table" and type(shot.data) == "string" then shot.data else ""
        if data == "" then
            error("Screenshot returned no image data", 0)
        end
        local meta = page(tab)
        return { text = `\{"tabID":{id(tab)},"url":{quote(meta.url)},"title":{quote(meta.title)},"format":"{format}"\}`, image = data, mimeType = `image/{format}` }
    end)

    -- The file tools exist only while an engine that keeps downloads runs (its Ready says so):
    -- an engine from before downloads would answer an empty list for ever.
    local filesAdded = false
    local function addFileTools()
        if filesAdded or not Browser:Engine().downloads then
            return
        end
        filesAdded = true
    tool("browser_files", "List the files a browser tab in OpenCode (Rune) downloaded (a click on a download link keeps the file for these tools until the tab closes; over 5 MiB is cancelled, at most 64 a tab), with each file's id, name, MIME type, size and state.", {
        tabID = tabSchema(DEFAULT_TAB),
    }, nil, function(args, sid)
        local tab = target(args, sid)
        return { tabID = id(tab), files = tab:Files() }
    end)

    tool("browser_file", "Read a completed download of a browser tab in OpenCode (Rune) by its id from browser_files: its text when it's UTF-8, else its bytes in base64. At most 5 MiB.", {
        tabID = tabSchema(DEFAULT_TAB),
        fileID = { type = "string", description = "File id from browser_files, e.g. 'file_1'." },
    }, { "fileID" }, function(args, sid)
        local tab = target(args, sid)
        local file = tab:ReadFileAsync(tostring(args.fileID))
        if file.text ~= nil then
            return { text = truncate(file.text) }
        end
        return { name = file.name, mime = file.mime, bytes = file.bytes, base64 = file.base64 }
    end)
    end
    addFileTools()
    Browser.Changed:Connect(addFileTools)
    tool("browser_cdp", "Send an allow-listed Chrome DevTools Protocol command to a browser tab in OpenCode (Rune).", {
        method = { type = "string", description = "Allow-listed CDP method name, e.g. 'Input.dispatchMouseEvent' or 'DOM.getDocument'." },
        params = { type = "object", description = "CDP method parameters." },
        tabID = tabSchema(DEFAULT_TAB),
        sessionId = { type = "string", description = "Optional CDP target sessionId." },
    }, { "method" }, function(args, sid)
        local method = if type(args.method) == "string" then args.method else ""
        if string.match(method, "^%s*$") then
            error("Missing 'method'", 0)
        end
        local tab = ready(target(args, sid))
        if not Browser:Engine().policy then
            error("browser_cdp requires an updated browser engine that enforces URL policy.", 0)
        end
        local result = tab:DevtoolsAsync(method, if type(args.params) == "table" then args.params else {}, if type(args.sessionId) == "string" then args.sessionId else nil)
        -- An answer that is one long `data` string is an image (a screenshot through CDP).
        if type(result) == "table" and type(result.data) == "string" and #result.data > 256 then
            local only = true
            for k in result do
                only = only and k == "data"
            end
            if only then
                local meta = page(tab)
                return { text = `\{"tabID":{id(tab)},"url":{quote(meta.url)},"title":{quote(meta.title)}\}`, image = result.data, mimeType = "image/png" }
            end
        end
        return { tabID = id(tab), result = compact(result) }
    end)
end

return mcp
