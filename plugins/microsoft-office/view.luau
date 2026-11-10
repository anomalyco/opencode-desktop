--!strict
-- What a viewer shows for each state of its file: a centred loader while it reads and opens
-- (`renderer.tsx`'s Suspense fallback), the file view's binary placeholder with why
-- (`artifact-view.tsx` `ArtifactBinary`), or the file: pages for a document or deck, a sheet and
-- its tabs for a workbook (`spreadsheet.tsx` `SpreadsheetWorkbook`).

local ui = require("@opencode/ui")
local machine = require("./machine")

local view = {}

export type Services = {
    I18n: I18n,
    Office: Office,
    redraw: () -> (),
}

-- A size as Desktop's file view writes one (`formatBytes`): 512 bytes, 12.3 kB, 4 MB.
function view.bytes(i18n: I18n, size: number): string
    local units = { "kB", "MB", "GB" }
    local index = if size > 0 then math.min(3, math.floor(math.log10(size) / 3)) else 0
    if index == 0 then
        return `{i18n:Number(size, { maxFraction = 0 })} {if size == 1 then "byte" else "bytes"}`
    end
    local value = size / 1000 ^ index
    return `{i18n:Number(value, { maxFraction = if value >= 100 then 0 else 1 })} {units[index]}`
end

function view.loading(): UiNode
    return ui.col({ key = "office-loading", w = "full", grow = 1, minH = 0, center = true }, { ui.loader() })
end

function view.binary(services: Services, path: string, size: number, reason: string?): UiNode
    local name = path:match("[^/\\]+$") or path
    local i18n = services.I18n
    -- `text-13-regular` names no style, so the lines keep the 24 px they inherit.
    local text = { size = 13, lineHeight = 24, color = "text-weak" :: Color }
    return ui.col({ key = "office-binary", w = "full", grow = 1, minH = 0, pad = 24, gap = 12, center = true, fill = "surface-1" }, {
        ui.icon(`file:{name}`, { size = 32, color = "text-weak" }),
        ui.text(name, { size = 14, weight = "medium", lineHeight = 21, color = "text-strong" }),
        ui.text(i18n:T("view.binary", { size = view.bytes(i18n, size) }), text),
        if reason then ui.text(i18n:T(reason), text) else nil,
    })
end

function view.pages(services: Services, document: OfficeDocument): UiNode
    local i18n = services.I18n
    local slides = document.kind == "pptx"
    return ui.officePages({
        key = "office-pages",
        w = "full",
        grow = 1,
        minH = 0,
        document = document,
        labels = {
            page = if slides then i18n:T("slide", { number = "{{number}}" }) else i18n:T("page", { number = "{{number}}" }),
            hiddenSlide = i18n:T("hiddenSlide"),
            failed = if slides then i18n:T("slideFailed") else i18n:T("document.pageFailed"),
            speakerNotes = i18n:T("speakerNotes"),
            layoutFailed = i18n:T("document.layoutFailed"),
        },
    })
end

-- A workbook: its sheet, and under it the tabs when it has more than one.
function view.workbook(services: Services, v: View, document: OfficeDocument, active: number, pick: (index: number) -> ()): UiNode
    local tabs: { UiNode } = {}
    if #document.sheets > 1 then
        for index, name in document.sheets do
            local selected = index == active
            table.insert(
                tabs,
                ui.row({
                    key = `office-sheet-tab-{index}`,
                    role = "tab",
                    label = name,
                    selected = selected,
                    h = 28,
                    px = 10,
                    radius = 6,
                    center = true,
                    fill = if selected then "surface-2" else nil,
                    hover = "surface-2",
                    hoverTransition = "instant",
                    onPress = v:Handler(function()
                        pick(index)
                    end),
                }, {
                    ui.text(name, { size = 13, lineHeight = 20, color = if selected then "text-strong" else "text-weak", noWrap = true }),
                })
            )
        end
    end
    return ui.col({ key = "office-workbook", w = "full", grow = 1, minH = 0 }, {
        ui.officeSheet({ key = `office-sheet-{active}`, w = "full", grow = 1, minH = 0, document = document }),
        if #tabs > 0 then ui.divider() else nil,
        if #tabs > 0
            then ui.row({
                key = "office-sheet-tabs",
                role = "group",
                label = services.I18n:T("sheet.tabs"),
                w = "full",
                h = 36,
                px = 8,
                gap = 4,
                align = "center",
                scrollX = true,
            }, tabs)
            else nil,
    })
end

function view.stage(services: Services, v: View, path: string, file: machine.File, active: number, pick: (index: number) -> ()): UiNode
    local status = file.status
    if status.kind == "reading" then
        return view.loading()
    elseif status.kind == "binary" then
        return view.binary(services, path, status.size, status.reason)
    elseif status.document.kind == "xlsx" then
        return view.workbook(services, v, status.document, active, pick)
    end
    return view.pages(services, status.document)
end

return view
