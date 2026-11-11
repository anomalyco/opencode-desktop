--!strict
-- Desktop's `SshCover` (`packages/gui-plugins/src/ssh/cover.tsx`): over a session or draft tab
-- of an SSH host that isn't connected, the lock, "SSH connection inactive", the host's name,
-- why to reconnect, the last error, and Reconnect (Authenticate while it waits for a sign-in;
-- a spinner and "Connecting to SSH server" while it connects). The tab stays mounted under it.

local ui = require("@opencode/ui")
local command = require("./command")

local cover = {}

local MUTED = { size = 13, weight = "regular", lineHeight = 20, color = "text-muted", textAlign = "center", wrap = true }

function cover.render(env: any, v: View, props: ServerCoverProps): UiNode?
    local item = env.item(props.id)
    if not item then
        return nil
    end
    local id = props.id
    local I18n = env.I18n
    -- Offers the sign-in once per server and visit to a tab (`cover.canOffer` / `cover.offer`):
    -- only while the host waits for a sign-in nobody answers and no dialog shows. Cancelling
    -- doesn't open it again on this visit; a later visit offers it again.
    if env.canOffer(id, props.visit, props.dialog) then
        env.offer(id, props.visit)
    end
    local connecting = env.pending(id)
    return v:Memo(`cover-{id}`, `{env.version()}|{props.tab}`, function()
        local status: { UiNode } = {
            ui.text(I18n:T("session.disconnected"), { size = 16, weight = "medium", lineHeight = 24, color = "text-chrome", textAlign = "center" }),
            ui.text(command.name(item.config), { size = 13, weight = "regular", lineHeight = 20, color = "text-muted", textAlign = "center", wrap = true, maxW = 384 }),
            ui.text(I18n:T("session.reconnectDescription"), { size = 13, weight = "regular", lineHeight = 20, color = "text-muted", textAlign = "center", wrap = true, maxW = 384 }),
        }
        local children: { UiNode } = {
            ui.icon("lock", { size = 20, color = "icon-weak" }),
            ui.col({ maxW = 384, gap = 8, align = "center", role = "status" }, status),
        }
        if item.error then
            table.insert(children, ui.text(I18n:T(`error.{item.error}`), MUTED))
        end
        local label = if connecting then I18n:T("session.connecting") elseif item.stage == "authentication" then I18n:T("action.authenticate") else I18n:T("session.reconnect")
        table.insert(children, ui.button({
            key = `ssh-cover-connect-{id}`,
            text = label,
            variant = "neutral",
            loading = connecting,
            disabled = connecting,
            onPress = v:Handler(function()
                env.connect(id)
            end),
        }))
        return ui.col({ key = `ssh-connection-panel-{id}`, w = "full", h = "full", fill = "bg-base", px = 24, py = 32, gap = 16, align = "center", justify = "center" }, children)
    end)
end

return cover
