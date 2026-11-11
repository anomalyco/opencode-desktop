--!strict
-- Desktop's built-in Microsoft Office plugin (`packages/gui-plugins/src/microsoft-office`):
-- read-only previews of Word, Excel and PowerPoint files in the file view. One viewer per kind,
-- as Desktop adds one `FileViewer` per kind. Bytes that are no Office Open XML package show the
-- binary placeholder with why; the rest open in the host (`Office:OpenAsync`), whose engines lay
-- them out on a thread of their own and draw their pages as vectors at the display's scale.

local plugin = require("@opencode/plugin")
local machine = require("@self/machine")
local view = require("@self/view")

return plugin.defineWindow(function(ctx)
    local I18n = ctx:GetService("I18n")
    local Office = ctx:GetService("Office")
    local changed = ctx:CreateSignal()
    local state = machine.new()
    local services: view.Services = { I18n = I18n, Office = Office, redraw = function()
        changed:Fire()
    end }

    local dispatch: (event: machine.Event) -> ()

    local function run(effect: machine.Effect)
        if effect.type == "open" then
            task.spawn(function()
                local file = Office:OpenAsync(effect.path, effect.directory, effect.kind)
                dispatch({ type = "read", path = effect.path, attempt = effect.attempt, file = file })
            end)
        elseif effect.type == "close" then
            Office:Close(effect.document)
        end
    end

    dispatch = function(event)
        for _, effect in machine.step(state, event) do
            run(effect)
        end
        changed:Fire()
    end

    local function viewer(id: string, title: string, kind: machine.Kind)
        ctx:AddFileViewer({
            id = id,
            title = title,
            extensions = { kind },
            render = function(v, props)
                local file = state.files[props.path]
                if not file or file.kind ~= kind or file.directory ~= props.directory then
                    -- Shown for the first time: read and open it after this render.
                    task.defer(dispatch, { type = "shown", path = props.path, directory = props.directory, kind = kind })
                    return view.loading()
                end
                machine.touch(state, props.path)
                local status = file.status
                local version = if status.kind == "open" then Office:Version(status.document) else 0
                -- The host's sheet: the tab picked, or the sheet a link in it went to.
                local active = if status.kind == "open" then Office:Sheet(status.document) else 1
                return v:Memo(`office-{props.path}`, `{status.kind}:{version}:{active}:{changed:Version()}:{I18n:Locale().id}`, function()
                    return view.stage(services, v, props.path, file, active, function(index)
                        if status.kind == "open" then
                            Office:SetSheet(status.document, index)
                            changed:Fire()
                        end
                    end)
                end)
            end,
        })
    end

    viewer("document", "Word", "docx")
    viewer("spreadsheet", "Excel", "xlsx")
    viewer("presentation", "PowerPoint", "pptx")

    ctx.Disposing:Connect(function()
        dispatch({ type = "disposed" })
    end)
end)
