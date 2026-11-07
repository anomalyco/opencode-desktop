--!strict
-- The app half: runs once per process, whatever the number of windows. It lists WSL distros as a
-- server source, starts `opencode serve` in a distro on demand, and shares a small API with the
-- window halves as a contract.

local plugin = require("@opencode/plugin")

type Distro = {
    name: string,
    process: ProcessHandle?,
    url: string?,
    state: ServerState,
}

local POLL_SECONDS = 30

return plugin.defineApp(function(ctx)
    local Process = ctx:GetService("Process")
    local Http = ctx:GetService("Http")
    local Store = ctx:GetService("Store")
    local Contracts = ctx:GetService("Contracts")
    local Console = ctx:GetService("Console")

    local autoStart = Store:Value("auto-start", false)
    local distros: { [string]: Distro } = {}
    local listed = false
    -- Declared ahead: the source's `connect` starts distros, and starting one publishes the source.
    local start: (distro: Distro) -> ()

    local source = ctx:AddServerSource({
        id = "distros",
        title = "WSL",
        order = 10,
        connect = function(id)
            local distro = distros[id]
            if not distro then
                return false
            end
            if distro.state ~= "ready" then
                start(distro)
                while distro.state == "starting" do
                    task.wait(0.25)
                end
            end
            return distro.state == "ready"
        end,
        remove = function(id)
            local distro = distros[id]
            if distro and distro.process then
                distro.process:Kill()
            end
        end,
    })

    local function publish()
        local entries: { ServerEntry } = {}
        for name, distro in distros do
            table.insert(entries, {
                id = name,
                name = name,
                label = "WSL",
                state = distro.state,
                http = if distro.url then { url = distro.url } else nil,
            })
        end
        table.sort(entries, function(a: ServerEntry, b: ServerEntry)
            return a.name < b.name
        end)
        source:SetEntries(entries, listed)
    end

    start = function(distro: Distro)
        if distro.process and distro.process:Running() then
            return
        end
        distro.state = "starting"
        publish()
        local process = Process:Spawn({
            program = "wsl.exe",
            args = { "-d", distro.name, "--", "opencode", "serve", "--port", "0" },
            encoding = "utf8",
        })
        distro.process = process
        process.Output:Connect(function(stream, text)
            local url = text:match("listening on (http://%S+)")
            if stream == "stdout" and url then
                distro.url = url
                distro.state = "ready"
                publish()
            end
        end)
        process.Exited:Connect(function(exit)
            distro.process = nil
            distro.url = nil
            distro.state = if exit.killed then "stopped" else "failed"
            Console:Warn(`{distro.name}: opencode exited`, exit.code, exit.error)
            publish()
        end)
    end

    local function refresh()
        local out = Process:RunAsync({
            program = "wsl.exe",
            args = { "--list", "--quiet" },
            encoding = "utf16le",
            timeout = 10,
        })
        if not out.ok then
            Console:Error("wsl.exe --list failed", out.stderr)
            return
        end
        local seen: { [string]: boolean } = {}
        for line in out.stdout:gmatch("[^\r\n]+") do
            local name = line:match("^%s*(.-)%s*$") or ""
            if #name > 0 then
                seen[name] = true
                distros[name] = distros[name] or { name = name, state = "stopped" }
            end
        end
        for name in distros do
            if not seen[name] then
                distros[name] = nil
            end
        end
        listed = true
        publish()
        if autoStart:Get() then
            for _, distro in distros do
                start(distro)
            end
        end
    end

    -- Checks each ready server's health on a timer; waiting costs no CPU.
    task.spawn(function()
        refresh()
        while true do
            task.wait(POLL_SECONDS)
            for _, distro in distros do
                if distro.url then
                    local res = Http:FetchAsync({ url = `{distro.url}/global/health`, timeout = 5 })
                    if not res.ok and distro.state == "ready" then
                        distro.state = "failed"
                        publish()
                    end
                end
            end
        end
    end)

    ctx:AddMenubarItem({
        menu = "file",
        id = "refresh",
        label = "Refresh WSL Distros",
        run = function()
            task.spawn(refresh)
        end,
    })

    Contracts:Provide("wsl-lite", {
        refresh = function()
            task.spawn(refresh)
        end,
        start = function(name: string)
            local distro = distros[name]
            if distro then
                start(distro)
            end
        end,
    })
end)
