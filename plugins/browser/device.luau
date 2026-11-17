--!strict
-- viewport-toolbar-machine.ts and viewport-controls-machine.ts at 1cb0633c8b. The
-- native page owns its applied viewport; the open controls own only their two editors.
local ui = require("@opencode/ui")
local presets = {
    {"galaxy-s26",360,780,"phone"}, {"iphone-18-pro",402,874,"phone"}, {"ipad-air-11",820,1180,"tablet"},
    {"laptop",1280,720,"desktop"}, {"desktop",1440,900,"desktop"}, {"iphone-se",375,667,"phone"},
    {"iphone-16",393,852,"phone"}, {"iphone-17e",390,844,"phone"}, {"iphone-16-pro-max",440,956,"phone"},
    {"iphone-duo-closed",466,678,"phone"}, {"iphone-duo-open",951,669,"phone"}, {"compact-phone",360,800,"phone"},
    {"pixel-10",412,924,"phone"}, {"pixel-9-pro-fold",412,922,"phone"}, {"ipad-mini-a20-pro",756,1158,"tablet"},
    {"ipad-pro-13",1032,1376,"tablet"}, {"surface-pro-10",960,1440,"windows_tablet"},
}

type Open = { status: "selected", tab: string }
type State = Open | {status: "following"}
local function step(state: State, tab: string, visible: boolean): (State, boolean)
    if visible then return {status="following"}, true end
    return {status="selected", tab=tab}, false
end

return function(ctx, i18n, menu, refresh)
    local state: State = {status="following"}
    local width = ctx:CreateEditor("")
    local height = ctx:CreateEditor("")
    local browser = ctx:GetService("Browser")
    local function label(key) return i18n:T("plugin.browser.viewport." .. key) end
    local function selected(tab)
        return (state.status == "selected" and state.tab == tab.Id) or tab:Viewport() ~= nil
    end
    local function toggle(tab)
        local reset
        state, reset = step(state, tab.Id, selected(tab))
        if reset then tab:SetViewport(nil) end
        refresh()
    end
    local function valid(value)
        local n = tonumber(value)
        return if n and string.match(value,"^%d+$") and n >= 1 and n <= 4096 then n else nil
    end
    local function commit()
        if state.status ~= "selected" then return end
        local tab = browser:Get(state.tab)
        local w,h = valid(width:Text()),valid(height:Text())
        if tab and w and h and tab:Zoom() == 1 then
            local prior = tab:Viewport()
            tab:SetViewport({width=w,height=h,profile="desktop",scale=if prior then prior.scale else nil})
        end
    end
    width.Submitted:Connect(commit)
    height.Submitted:Connect(commit)
    local function button(view,key,words,help,press)
        return ui.row({key=key,role="button",help=help,label=help,h=24,px=6,center=true,radius=4,
            hover="surface-hover",onPress=view:Handler(press)}, {ui.text(words,{size=12,color="text-chrome"})})
    end
    local function render(view,tab,props)
        if not selected(tab) then return nil end
        if tab:Zoom() ~= 1 then return ui.text(label("zoomPrompt"), {size=12,color="text-muted"}) end
        local d = tab:Viewport()
        local w = if d then d.width else math.max(1,math.floor(props.width))
        local h = if d then d.height else math.max(1,math.floor(props.height-80))
        local function field(axis,editor,n)
            if not editor:Focused() and editor:Text() ~= tostring(n) then editor:SetText(tostring(n)) end
            return ui.field({key="browser-viewport-"..axis,editor=editor,variant="bare",w=36,h=24,label=label(axis)})
        end
        local name = if d and d.preset then label("device." .. d.preset) else label(if d then "custom" else "responsive")
        local selector = button(view,"browser-device-preset",name,label("preset"),function()
            local items = {{id="auto",title=label("responsive")}}
            for _,p in presets do table.insert(items,{id=p[1],title=label("device." .. p[1])}) end
            local picked = menu:OpenAsync({anchorKey="browser-device-preset",label=label("preset"),items=items})
            if picked == "auto" then tab:SetViewport(nil) end
            for _,p in presets do
                if p[1] == picked then tab:SetViewport({preset=p[1],width=p[2],height=p[3],profile=p[4],scale=if d then d.scale else nil}) end
            end
        end)
        local scale = button(view,"browser-device-scale",if d and d.scale then `{d.scale*100}%` else label("fit"),label("scale"),function()
            local items = {{id="fit",title=label("fit")}}
            for _,n in {0.5,0.75,1,1.25,1.5,2} do table.insert(items,{id=tostring(n),title=`{n*100}%`}) end
            local picked = menu:OpenAsync({anchorKey="browser-device-scale",label=label("scale"),items=items})
            if not picked then return end
            local next = if d then table.clone(d) else {width=w,height=h,profile="desktop"}
            next.scale = tonumber(picked)
            tab:SetViewport(next)
        end)
        return ui.row({key="browser-device-toolbar",w="full",h=36,gap=4,align="center",justify="center",fill="surface-1"},{
            selector,field("width",width,w),ui.text("×",{size=12,color="text-muted"}),field("height",height,h),
            button(view,"browser-device-rotate","↻",label("rotate"),function()
                local next = if d then table.clone(d) else {profile="desktop"}
                next.width,next.height=h,w
                tab:SetViewport(next)
            end),scale,
        })
    end
    return {toggle=toggle,render=render}
end
