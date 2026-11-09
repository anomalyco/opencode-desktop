--!strict
-- Rune UI: a reply's ```` ```ui ```` outlines draw as parts (frames, rows, chat threads, facts,
-- statuses and controls) rather than as code. The host keeps the outline parser and its animated
-- widgets (`ui.genUi`); turning this plugin off shows the outlines as the code they are.

local plugin = require("@opencode/plugin")
local ui = require("@opencode/ui")

return plugin.defineWindow(function(ctx)
    ctx:AddCodeBlock({
        id = "outline",
        languages = { "ui" },
        render = function(_view, props)
            return ui.genUi({ source = props.code, streaming = props.streaming })
        end,
    })
end)
