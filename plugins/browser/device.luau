--!strict
-- Device emulation toolbar matching upstream `UP-039` (#63819):
-- `viewport-toolbar-machine.ts`, `viewport-controls-machine.ts`, `viewport-presets.ts`,
-- `viewport-preset-menu.tsx`, and `viewport-controls.tsx`.
-- Uses existing bounds-checked `BrowserTab:SetViewport` (`HostMsg::Emulate`, #245).

local ui = require("@opencode/ui")

export type Preset = {
    id: string,
    width: number,
    height: number,
    profile: "phone" | "tablet" | "windows_tablet" | "desktop",
}

local QUICK_PRESETS: { Preset } = {
    { id = "galaxy-s26", width = 360, height = 780, profile = "phone" },
    { id = "iphone-18-pro", width = 402, height = 874, profile = "phone" },
    { id = "ipad-air-11", width = 820, height = 1180, profile = "tablet" },
    { id = "laptop", width = 1280, height = 720, profile = "desktop" },
    { id = "desktop", width = 1440, height = 900, profile = "desktop" },
}

local MORE_DEVICES: { Preset } = {
    { id = "iphone-se", width = 375, height = 667, profile = "phone" },
    { id = "iphone-16", width = 393, height = 852, profile = "phone" },
    { id = "iphone-17e", width = 390, height = 844, profile = "phone" },
    { id = "iphone-16-pro-max", width = 440, height = 956, profile = "phone" },
    { id = "iphone-duo-closed", width = 466, height = 678, profile = "phone" },
    { id = "iphone-duo-open", width = 951, height = 669, profile = "phone" },
    { id = "compact-phone", width = 360, height = 800, profile = "phone" },
    { id = "pixel-10", width = 412, height = 924, profile = "phone" },
    { id = "pixel-9-pro-fold", width = 412, height = 922, profile = "phone" },
    { id = "ipad-mini-a20-pro", width = 756, height = 1158, profile = "tablet" },
    { id = "ipad-pro-13", width = 1032, height = 1376, profile = "tablet" },
    { id = "surface-pro-10", width = 960, height = 1440, profile = "windows_tablet" },
}

local SCALES: { number } = { 0.5, 0.75, 1, 1.25, 1.5, 2 }

type ToolbarState = { status: "following" } | { status: "selected", tab: string }

local function stepToolbar(state: ToolbarState, tabId: string, open: boolean): (ToolbarState, boolean)
    if open then
        return { status = "following" }, true
    end
    return { status = "selected", tab = tabId }, false
end

local function validDimension(value: string): number?
    if string.match(value, "^%d+$") == nil then
        return nil
    end
    local n = tonumber(value)
    if n and n >= 1 and n <= 4096 then
        return n
    end
    return nil
end

local function effectiveScale(d: BrowserViewport?, areaW: number, areaH: number): number
    if d == nil then
        return 1
    end
    if d.scale ~= nil then
        return d.scale
    end
    local aw = math.max(1, math.floor(areaW - 16))
    local ah = math.max(1, math.floor(areaH - 16))
    return math.min(1, aw / d.width, ah / d.height)
end

local function findPreset(id: string): Preset?
    for _, p in QUICK_PRESETS do
        if p.id == id then
            return p
        end
    end
    for _, p in MORE_DEVICES do
        if p.id == id then
            return p
        end
    end
    return nil
end

local ROW_TEXT = table.freeze({ size = 13, weight = "regular", lineHeight = 16, tracking = -0.04, noWrap = true })

export type Controller = {
    isOpen: (tab: BrowserTab) -> boolean,
    toggle: (tab: BrowserTab) -> (),
    render: (view: View, tab: BrowserTab, props: PanelProps) -> ui.Node?,
}

return function(ctx: any, i18n: I18nService, popover: PopoverService, refresh: () -> ()): Controller
    local state: ToolbarState = { status = "following" }
    local draftTab: string? = nil
    local draftW: string? = nil
    local draftH: string? = nil
    local widthEd = ctx:CreateEditor("")
    local heightEd = ctx:CreateEditor("")
    local browser = ctx:GetService("Browser")

    local function t(key: string, params: { [string]: any }?): string
        return i18n:T(`viewport.{key}`, params)
    end

    local function isOpen(tab: BrowserTab): boolean
        return (state.status == "selected" and state.tab == tab.Id) or tab:Viewport() ~= nil
    end

    local function activeTab(): BrowserTab?
        if state.status == "selected" then
            return browser:Get(state.tab)
        end
        return browser:Selected(nil)
    end

    local function clearDraft()
        draftTab = nil
        draftW = nil
        draftH = nil
    end

    local function applyDimensions(tab: BrowserTab, wText: string, hText: string)
        if math.abs(tab:Zoom() - 1) > 0.001 then
            return
        end
        local w = validDimension(wText)
        local h = validDimension(hText)
        if w == nil or h == nil then
            return
        end
        local prior = tab:Viewport()
        tab:SetViewport({
            width = w,
            height = h,
            profile = "desktop",
            scale = if prior then prior.scale else nil,
        })
    end

    local function onEdit(axis: "width" | "height", value: string)
        local tab = activeTab()
        if tab == nil then
            return
        end
        if draftTab ~= tab.Id then
            local d = tab:Viewport()
            draftTab = tab.Id
            draftW = if d then tostring(d.width) else widthEd:Text()
            draftH = if d then tostring(d.height) else heightEd:Text()
        end
        if axis == "width" then
            draftW = value
        else
            draftH = value
        end
        applyDimensions(tab, draftW or "", draftH or "")
        refresh()
    end

    widthEd.Changed:Connect(function(val: string)
        if widthEd:Focused() then
            onEdit("width", val)
        end
    end)
    heightEd.Changed:Connect(function(val: string)
        if heightEd:Focused() then
            onEdit("height", val)
        end
    end)
    widthEd.Submitted:Connect(function()
        local tab = activeTab()
        if tab then
            applyDimensions(tab, widthEd:Text(), heightEd:Text())
        end
    end)
    heightEd.Submitted:Connect(function()
        local tab = activeTab()
        if tab then
            applyDimensions(tab, widthEd:Text(), heightEd:Text())
        end
    end)
    local function onBlur()
        if not widthEd:Focused() and not heightEd:Focused() then
            clearDraft()
            refresh()
        end
    end
    widthEd.FocusChanged:Connect(function(focused: boolean)
        if not focused then
            onBlur()
        else
            refresh()
        end
    end)
    heightEd.FocusChanged:Connect(function(focused: boolean)
        if not focused then
            onBlur()
        else
            refresh()
        end
    end)

    local function selectPreset(tabId: string, presetId: string?)
        popover:Close()
        local tab = browser:Get(tabId)
        if tab == nil or math.abs(tab:Zoom() - 1) > 0.001 then
            return
        end
        clearDraft()
        if presetId == nil then
            tab:SetViewport(nil)
            refresh()
            return
        end
        local p = findPreset(presetId)
        if p == nil then
            return
        end
        local prior = tab:Viewport()
        tab:SetViewport({
            preset = p.id,
            width = p.width,
            height = p.height,
            profile = p.profile,
            scale = if prior then prior.scale else nil,
        })
        refresh()
    end

    local function selectScale(tabId: string, scaleValue: number?, autoW: number, autoH: number)
        popover:Close()
        local tab = browser:Get(tabId)
        if tab == nil or math.abs(tab:Zoom() - 1) > 0.001 then
            return
        end
        local prior = tab:Viewport()
        if prior == nil and scaleValue == nil then
            return
        end
        local nextViewport: BrowserViewport = if prior
            then table.clone(prior)
            else { width = autoW, height = autoH, profile = "desktop" }
        nextViewport.scale = scaleValue
        tab:SetViewport(nextViewport)
        refresh()
    end

    local function checkRow(view: View, key: string, title: string, checked: boolean, press: () -> (), rowH: number?, rowRadius: number?): ui.Node
        local children: { ui.Node } = {
            ui.text(title, ROW_TEXT, { color = "text-chrome", grow = 1, shrink = true, minW = 0, truncate = true }),
        }
        if checked then
            table.insert(children, ui.icon("check", { size = 14, color = "icon-base" }))
        end
        return ui.row({
            key = key,
            role = "radio",
            label = title,
            w = "full",
            h = rowH or 28,
            pl = 10,
            pr = 8,
            gap = 8,
            radius = rowRadius or 4,
            align = "center",
            hover = "surface-hover",
            press = "surface-press",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                press()
            end),
        }, children)
    end

    -- Preset dropdown popover (`viewport-preset-menu.tsx`, `bar.css:337-363`).
    ctx:AddPopover({
        id = "viewport-preset",
        title = t("preset"),
        render = function(view, props)
            local data: { [string]: any } = props.data or {}
            local tabId = tostring(data.tabId or "")
            local selected = data.selected :: string?
            local items: { ui.Node } = {
                checkRow(view, "vp-auto", t("responsive"), selected == nil, function()
                    selectPreset(tabId, nil)
                end, 30, 5),
            }
            if selected == "custom" then
                table.insert(items, checkRow(view, "vp-custom", t("custom"), true, function()
                    popover:Close()
                end, 30, 5))
            end
            for _, p in QUICK_PRESETS do
                table.insert(items, checkRow(view, `vp-{p.id}`, t(`device.{p.id}`), selected == p.id, function()
                    selectPreset(tabId, p.id)
                end, 30, 5))
            end
            table.insert(items, ui.col({ w = "full", h = 1, my = 4, fill = "border-weak" }))
            table.insert(items, ui.flyoutRow({
                key = "vp-more-devices",
                text = t("moreDevices"),
                popover = "viewport-more-devices",
                side = "right",
                h = 30,
                pl = 10,
                pr = 8,
                radius = 5,
                props = data,
            }))
            return ui.col({
                key = "browser-viewport-preset-menu",
                role = "list",
                label = t("preset"),
                w = math.min(224, math.max(160, (tonumber(data.paneW) or 400) - 16)),
                pad = 4,
                radius = 10,
                fill = "bg-base",
                shadow = "floating",
            }, items)
        end,
    })

    ctx:AddPopover({
        id = "viewport-more-devices",
        title = t("moreDevices"),
        render = function(view, props)
            local data: { [string]: any } = props.data or {}
            local tabId = tostring(data.tabId or "")
            local selected = data.selected :: string?
            local items: { ui.Node } = {}
            for _, p in MORE_DEVICES do
                table.insert(items, checkRow(view, `vp-{p.id}`, t(`device.{p.id}`), selected == p.id, function()
                    selectPreset(tabId, p.id)
                end, 30, 5))
            end
            return ui.col({
                key = "browser-viewport-more-menu",
                role = "list",
                label = t("moreDevices"),
                minW = 224,
                maxH = 360,
                scrollY = true,
                pad = 4,
                radius = 10,
                fill = "bg-base",
                shadow = "floating",
            }, items)
        end,
    })

    -- Scale dropdown popover (`viewport-controls.tsx:112-146`, `bar.css:327-335`).
    ctx:AddPopover({
        id = "viewport-scale",
        title = t("scale"),
        render = function(view, props)
            local data: { [string]: any } = props.data or {}
            local tabId = tostring(data.tabId or "")
            local currentScale = data.scale :: number?
            local autoW = tonumber(data.autoW) or 800
            local autoH = tonumber(data.autoH) or 600
            local items: { ui.Node } = {
                checkRow(view, "vp-scale-fit", t("fit"), currentScale == nil, function()
                    selectScale(tabId, nil, autoW, autoH)
                end),
                ui.col({ w = "full", h = 1, my = 4, fill = "border-weak" }),
            }
            for _, factor in SCALES do
                local pct = i18n:T("zoom.percent", { percent = math.floor(factor * 100 + 0.5) })
                local matched = currentScale ~= nil and math.abs(currentScale - factor) < 0.001
                table.insert(items, checkRow(view, `vp-scale-{factor}`, pct, matched, function()
                    selectScale(tabId, factor, autoW, autoH)
                end))
            end
            return ui.col({
                key = "browser-viewport-scale-menu",
                role = "list",
                label = t("scale"),
                w = 162,
                pad = 4,
                radius = 9,
                fill = "surface-1",
                shadow = "floating",
            }, items)
        end,
    })

    local function toggle(tab: BrowserTab)
        local reset: boolean
        state, reset = stepToolbar(state, tab.Id, isOpen(tab))
        clearDraft()
        if reset then
            tab:SetViewport(nil)
        end
        refresh()
    end

    local function render(view: View, tab: BrowserTab, props: PanelProps): ui.Node?
        if not isOpen(tab) then
            return nil
        end
        local zoomed = math.abs(tab:Zoom() - 1) > 0.001
        local d = tab:Viewport()
        -- Reserve the 45 pt address bar + 36 pt device bar (81 pt) and 14 pt resize gutters (`bar.css:195-204`).
        local areaW = math.max(1, math.floor(props.width - 14))
        local areaH = math.max(1, math.floor(props.height - 81 - 14))
        local wNum = if d then d.width else areaW
        local hNum = if d then d.height else areaH
        local wStr = if draftTab == tab.Id and draftW ~= nil then draftW else tostring(wNum)
        local hStr = if draftTab == tab.Id and draftH ~= nil then draftH else tostring(hNum)
        local invalidW = validDimension(wStr) == nil
        local invalidH = validDimension(hStr) == nil
        local invalid = invalidW or invalidH

        if not widthEd:Focused() and widthEd:Text() ~= wStr then
            widthEd:SetText(wStr)
        end
        if not heightEd:Focused() and heightEd:Text() ~= hStr then
            heightEd:SetText(hStr)
        end

        local selected: string? = if d then (d.preset or "custom") else nil
        local presetLabel = if selected == "custom" then t("custom") elseif selected then t(`device.{selected}`) else t("responsive")
        local eff = effectiveScale(d, props.width - 14, props.height - 81 - 14)
        local scalePct = math.floor(eff * 100 + 0.5)
        local scaleLabel = if d and d.scale ~= nil
            then i18n:T("zoom.percent", { percent = math.floor(d.scale * 100 + 0.5) })
            elseif eff < 0.01 then t("fitBelowOne")
            else t("fitPercent", { percent = scalePct })

        local presetOpen = popover:IsOpen("viewport-preset")
        local presetBtn = ui.row({
            key = "browser-device-preset",
            role = "button",
            label = t("preset"),
            help = presetLabel,
            h = 24,
            pl = 8,
            pr = 4,
            gap = 4,
            radius = 4,
            align = "center",
            fill = if presetOpen then "surface-press" else nil,
            hover = if not zoomed and not presetOpen then "surface-hover" else nil,
            press = if not zoomed then "surface-press" else nil,
            hoverTransition = "instant",
            opacity = if zoomed then 0.5 else nil,
            onPress = if not zoomed then view:Handler(function()
                popover:Toggle({
                    id = "viewport-preset",
                    anchorKey = "browser-device-preset",
                    side = "bottom",
                    align = "start",
                    gap = 4,
                    data = { tabId = tab.Id, selected = selected, paneW = props.width },
                })
            end) else nil,
        }, {
            ui.text(presetLabel, { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, noWrap = true, color = "text-chrome", truncate = true, shrink = true, minW = 0 }),
            ui.icon("viewport-chevron", { size = 16, color = "icon-weak" }),
        })

        local scaleOpen = popover:IsOpen("viewport-scale")
        local scaleBtn = ui.row({
            key = "browser-device-scale",
            role = "button",
            label = t("scale"),
            help = scaleLabel,
            h = 24,
            pl = 8,
            pr = 4,
            gap = 4,
            radius = 4,
            align = "center",
            fill = if scaleOpen then "surface-press" else nil,
            hover = if not zoomed and not scaleOpen then "surface-hover" else nil,
            press = if not zoomed then "surface-press" else nil,
            hoverTransition = "instant",
            opacity = if zoomed then 0.5 else nil,
            onPress = if not zoomed then view:Handler(function()
                popover:Toggle({
                    id = "viewport-scale",
                    anchorKey = "browser-device-scale",
                    side = "bottom",
                    align = "end",
                    gap = 4,
                    data = { tabId = tab.Id, scale = if d then d.scale else nil, autoW = areaW, autoH = areaH },
                })
            end) else nil,
        }, {
            ui.text(scaleLabel, { size = 13, weight = "regular", lineHeight = 16, tracking = -0.04, noWrap = true, color = "text-faint" }),
            ui.icon("viewport-chevron", { size = 16, color = "icon-weak" }),
        })

        local rotateEnabled = not invalid and not zoomed
        local rotateBtn = ui.iconButton({
            key = "browser-device-rotate",
            icon = "viewport-rotate",
            variant = "ghost-muted",
            size = "small",
            label = t("rotate"),
            help = t("rotate"),
            helpSide = "top",
            disabled = not rotateEnabled,
            onPress = if rotateEnabled then view:Handler(function()
                local nextW = validDimension(hStr) or hNum
                local nextH = validDimension(wStr) or wNum
                clearDraft()
                local nextViewport: BrowserViewport = if d then table.clone(d) else { width = nextW, height = nextH, profile = "desktop" }
                nextViewport.width = nextW
                nextViewport.height = nextH
                tab:SetViewport(nextViewport)
                refresh()
            end) else nil,
        })

        local function dimBox(key: string, ed: Editor, val: string, isInvalid: boolean, ariaLabel: string): ui.Node
            local editing = ed:Focused() and not zoomed
            local body: ui.Node = if editing
                then ui.field({
                    key = `{key}-input`,
                    editor = ed,
                    variant = "bare",
                    dir = "ltr",
                    disabled = zoomed,
                    label = ariaLabel,
                    grow = 1,
                })
                else ui.text(val, {
                    size = 13,
                    weight = "regular",
                    lineHeight = 16,
                    tabular = true,
                    noWrap = true,
                    color = if isInvalid and not zoomed then "status-danger" else "text-chrome",
                })
            return ui.row({
                key = key,
                role = if editing then nil else "button",
                label = ariaLabel,
                dir = "ltr",
                w = 50,
                h = 24,
                px = if editing then 6 else 4,
                radius = 6,
                center = not editing,
                align = "center",
                fill = "bg-base",
                shadow = if editing or (isInvalid and not zoomed) then nil else "raised",
                border = if isInvalid and not zoomed
                    then "status-danger"
                    elseif editing then "border-focus"
                    else nil,
                borderWidth = if editing or (isInvalid and not zoomed) then 1 else nil,
                opacity = if zoomed then 0.5 else nil,
                onPress = if not zoomed and not editing then view:Handler(function()
                    ed:Focus()
                    refresh()
                end) else nil,
            }, { body })
        end

        local dimGroup = ui.row({ role = "group", label = t("dimensions"), gap = 6, align = "center" }, {
            dimBox("browser-viewport-width", widthEd, wStr, invalidW, t("width")),
            ui.text("×", { size = 13, color = "text-faint", noWrap = true }),
            dimBox("browser-viewport-height", heightEd, hStr, invalidH, t("height")),
        })

        local controlsRow = ui.row({ w = "full", minH = 35, px = 6, py = 5, gap = 12, align = "center", justify = "center", wrap = true }, {
            presetBtn,
            dimGroup,
            ui.row({ gap = 6, align = "center" }, { scaleBtn, rotateBtn }),
        })

        local rows: { ui.Node } = { controlsRow }
        if zoomed then
            table.insert(rows, ui.row({ key = "browser-viewport-zoom-notice", role = "status", w = "full", px = 12, pb = 6, gap = 8, align = "center", justify = "center" }, {
                ui.text(t("zoomPrompt"), { size = 12, lineHeight = 16, color = "text-muted" }),
                ui.button({
                    key = "browser-viewport-reset-zoom",
                    text = i18n:T("zoom.reset"),
                    size = "small",
                    variant = "ghost",
                    onPress = view:Handler(function()
                        tab:SetZoom("reset")
                    end),
                }),
            }))
        elseif invalid then
            table.insert(rows, ui.row({ key = "browser-viewport-invalid", role = "status", w = "full", px = 12, pb = 6, gap = 8, align = "center", justify = "center" }, {
                ui.text(t("invalid"), { size = 12, lineHeight = 16, color = "status-danger" }),
            }))
        end

        return ui.col({ key = "browser-device-toolbar", role = "region", label = t("label"), w = "full", fill = "bg-base" }, {
            ui.col({ w = "full" }, rows),
            ui.col({ w = "full", h = 1, fill = "border-weak" }),
        })
    end

    return { isOpen = isOpen, toggle = toggle, render = render }
end
