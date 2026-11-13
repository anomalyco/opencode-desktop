--!strict
-- An SSH server's row in Settings › Servers (Desktop's `row.tsx`): its health, its name and the
-- SSH badge, its version (or Authentication required), Authenticate while it waits for a
-- sign-in, and its menu (Connect or Authenticate, Delete).

local ui = require("@opencode/ui")
local command = require("./command")

local row = {}

function row.render(env: any, v: View, props: ServerRowProps): UiNode?
    local item = env.item(props.id)
    if not item then
        return nil
    end
    local id = props.id
    local I18n = env.I18n
    local pending = env.pending(id)
    return v:Memo(`row-{id}`, `{env.version()}|{props.health and props.health.version or ""}`, function()
        local stage = item.stage
        local health = nil
        if stage == "ready" then
            health = props.health or { healthy = true }
        elseif stage == "failed" then
            health = { healthy = false }
        end
        local meta = if stage == "authentication" then I18n:T("stage.authentication") elseif props.health and props.health.version then `v{props.health.version}` else ""
        local lead = ui.row({ grow = 1, shrink = true, minW = 0, gap = 10 }, {
            ui.serverIndicator({ health = health, connecting = stage == "connecting", auth = stage == "authentication" }),
            ui.col({ grow = 1, shrink = true, minW = 0, gap = 6 }, {
                ui.row({ gap = 4, align = "center", minW = 0 }, {
                    -- A given name reads its own way; the host taken from the target stays left to right.
            ui.text(command.name(item.config), { size = 13, weight = "medium", lineHeight = 16, color = "text-chrome", truncate = true, shrink = true, dir = if (item.config.name or "") ~= "" then "auto" else "ltr" }),
                    ui.row({ px = 4, py = 2, radius = 3, border = "border-base", borderWidth = 1 }, {
                        ui.text(I18n:T("label"), { size = 9, weight = "regular", lineHeight = 9, color = "text-muted" }),
                    }),
                }),
                ui.text(meta, { size = 11, weight = "regular", lineHeight = 11, color = "text-muted" }),
            }),
        })
        local actions: { UiNode } = {}
        if stage == "authentication" or pending then
            table.insert(actions, ui.button({
                key = `ssh-row-auth-{id}`,
                text = if pending then I18n:T("session.connecting") else I18n:T("action.authenticate"),
                variant = "ghost-muted",
                size = "small",
                loading = pending,
                disabled = pending,
                onPress = v:Handler(function()
                    env.connect(id)
                end),
            }))
        end
        local anchor = `ssh-row-menu-{id}`
        table.insert(actions, ui.iconButton({
            key = anchor,
            icon = "outline-dots",
            variant = "ghost-muted",
            size = "small",
            label = I18n:T("common.moreOptions"),
            onPress = v:Handler(function()
                local now = env.item(id)
                if not now then
                    return
                end
                local items: { MenuEntry } = { { kind = "group", title = I18n:T("server.menu.label") } }
                if now.stage == "authentication" then
                    table.insert(items, { id = "connect", title = I18n:T("authenticate"), enabled = not env.pending(id) })
                elseif now.stage ~= "ready" then
                    table.insert(items, { id = "connect", title = I18n:T("connect"), enabled = not env.pending(id) })
                end
                table.insert(items, { kind = "separator" })
                table.insert(items, { id = "delete", title = I18n:T("menu.delete") })
                local picked = env.Menu:OpenAsync({ anchorKey = anchor, items = items, label = I18n:T("server.menu.label"), align = "end" })
                if picked == "connect" then
                    env.connect(id)
                elseif picked == "delete" then
                    env.forget(id)
                end
            end),
        }))
        return ui.row({ key = `ssh-row-{id}`, w = "full", gap = 16, align = "center" }, {
            lead,
            ui.row({ gap = 8, align = "center" }, actions),
        })
    end)
end

return row
