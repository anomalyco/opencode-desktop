--!strict
-- The Files tab (browser.tsx, SessionFilePanelV2): the sidebar with the folder's name, Open in,
-- a filter and the workspace tree (or the files the filter finds), its change letters from the
-- review; beside it the open file. With the sidebar shut, a toolbar with the name and Open in;
-- the Open file launcher keeps the sidebar open and shows what to do in place of a file.

local ui = require("@opencode/ui")

local panel = {}

-- The Open file launcher's instance: its tab keys as `file:open`.
panel.OPEN = "open"

export type Env = {
    I18n: I18nService,
    Layout: LayoutService,
    Workspace: WorkspaceService,
    -- The review's `review.changes`: each changed path's kind, for the tree's letters.
    changes: Live<any>,
    -- The sidebar's filter.
    filter: Editor,
}

local function isLauncher(id: string): boolean
    return id == panel.OPEN or string.sub(id, 1, #panel.OPEN + 1) == `{panel.OPEN}:`
end

-- The path a file tab shows (`//<path>`), nil for a launcher.
function panel.path(id: string): string?
    if string.sub(id, 1, 2) == "//" and #id > 2 then
        return string.sub(id, 3)
    end
    return nil
end

local function basename(path: string): string
    return string.match(path, "([^/\\]+)[/\\]*$") or path
end

-- The open tabs: the launcher (transient, not draggable) and one per file, named for it or as
-- not found; a gone selection falls back to a file tab.
function panel.tabs(env: Env, sid: string, open: { string }): { PanelTab }
    local out: { PanelTab } = {}
    for _, id in open do
        local path = panel.path(id)
        if isLauncher(id) then
            table.insert(out, { instance = id, title = env.I18n:T("command.open"), icon = "file-tree", transient = true, draggable = false })
        elseif path then
            local name = basename(path)
            local missing = env.Workspace:OpenDocument(path):Missing()
            table.insert(out, {
                instance = id,
                title = if missing then env.I18n:T("tab.notFound", { name = name }) else name,
                icon = `file:{name}` :: any,
                file = path,
                missing = missing,
                fallback = true,
            })
        end
    end
    return out
end

-- Opens `path` in a kept tab (or browser preview for HTML files), in place of the launcher.
function panel.open(env: Env, sid: string, path: string)
    env.Layout:ClosePanel("main", { sessionId = sid, instance = panel.OPEN })
    env.Workspace:OpenFile(sid, path, { preview = false })
end

-- The first file the filter finds for `text` in `directory`.
function panel.found(env: Env, directory: string, text: string): string?
    local query = string.gsub(text, "^%s*(.-)%s*$", "%1")
    if query == "" then
        return nil
    end
    return env.Workspace:OpenTree(directory):Find(query)[1]
end

-- Each changed path's kind, while the review provides them.
local function kinds(env: Env, sid: string): { [string]: ChangeKind }?
    local changes = env.changes:Get()
    if changes == nil then
        return nil
    end
    return changes.kinds(sid)
end

local function kindsVersion(env: Env, sid: string): string
    local gen = env.changes:Generation()
    local changes = env.changes:Get()
    if changes == nil then
        return `{gen}:nil`
    end
    local kv = if changes.kindsVersion then changes.kindsVersion(sid) else 0
    return `{gen}:{kv}`
end

-- The folder's name (`title()`): the last part of the session's folder.
local function title(env: Env, directory: string): ui.Node
    local name = basename(directory)
    if name == "" then
        name = env.I18n:T("rune.files.workspace")
    end
    return ui.text(name, { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text-strong", fadeOut = true, shrink = true, minW = 0 })
end

-- `session-review-v2-sidebar`: its header 48 tall, 54 in past the toggle (gap 10), Open in at its
-- end for a file (not the launcher), the filter, then the tree or what the filter finds. With the
-- sidebar on the end edge the toggle is at the header's end (`data-sidebar-side="end"`: `8px 54px`,
-- and the title, no select, 8 further in).
local function sidebar(env: Env, view: View, props: PanelProps, tree: FileTreeModel, path: string?, side: PanelSidebarSide): ui.Node
    local locale = env.I18n:Locale().id
    local look = `{locale}:{tostring(view.dark)}`
    local atEnd = side == "end"
    local header = view:Memo("files-sidebar-header-slot", `{props.directory}:{path or ""}:{side}:{look}`, function()
        local items: { ui.Node } = { title(env, props.directory), ui.spacer() }
        if path then
            table.insert(items, ui.openInButton({ key = "files-open-in", path = path }))
        end
        return ui.col({ w = "full" }, {
            ui.row({ key = "files-sidebar-header", w = "full", h = 48, pl = if atEnd then 16 else 54, pr = if atEnd then 54 else 16, gap = 8, align = "center" }, items),
        })
    end)
    local filter = view:Memo("files-filter-slot", `{env.filter:Version()}:{look}`, function()
        return ui.col({ w = "full" }, {
            ui.field({ key = "files-filter", editor = env.filter, variant = "filter", placeholder = env.I18n:T("ui.sessionReviewV2.filterFiles") }),
        })
    end)
    local query = env.filter:Text()
    local kv = kindsVersion(env, props.sessionId)
    local list = view:Memo("files-tree-slot", `{props.sessionId}:{path or ""}:{tree:Version()}:{query}:{kv}:{look}`, function()
        return ui.col({ w = "full", grow = 1, minH = 0 }, {
            ui.fileTree({ tree = tree, filter = query, kinds = kinds(env, props.sessionId), selectedPath = path, pt = 4, grow = 1 }),
        })
    end)
    return ui.col({ w = "full", h = "full" }, { header, filter, list })
end

-- `SessionFilePanelV2Empty` for the launcher: the file-tree icon, Open file and its hint.
local function empty(env: Env, view: View): ui.Node
    return view:Memo("files-empty-slot", `{env.I18n:Locale().id}:{tostring(view.dark)}`, function()
        return ui.col({ w = "full", grow = 1 }, {
            ui.col({ key = "files-placeholder", w = "full", grow = 1, align = "center", pad = 24 }, {
                ui.icon("file-tree", { size = 20, color = "text-weak" }),
                ui.col({ h = 16 }, {}),
                ui.text(env.I18n:T("command.open"), { size = 13, weight = "medium", lineHeight = 13, color = "text-strong" }),
                ui.col({ h = 8 }, {}),
                ui.row({ h = 20, align = "center" }, {
                    ui.text(env.I18n:T("selectToOpen"), { size = 13, lineHeight = 20, tracking = -0.04, color = "text-weak" }),
                }),
            }),
        })
    end)
end

function panel.wide(env: Env, view: View, props: PanelProps): ui.Node
    local id = props.instance or panel.OPEN
    local path = panel.path(id)
    local launcher = path == nil
    local tree = env.Workspace:OpenTree(props.directory)
    local bar = env.Layout:PanelSidebar()
    local opened = bar.opened or launcher
    local atEnd = bar.side == "end"
    local main: { ui.Node } = {}
    -- `session-review-v2-toolbar` while the sidebar is shut: the name and Open in, 54 in past the
    -- toggle, which is at its end with the sidebar on the end edge (`8px 54px`).
    if not opened then
        local look = `{env.I18n:Locale().id}:{tostring(view.dark)}`
        table.insert(main, view:Memo("files-shut-toolbar-slot", `{props.directory}:{path or ""}:{bar.side}:{look}`, function()
            return ui.col({ w = "full" }, {
                ui.row({ key = "files-toolbar", w = "full", h = 47, pl = if atEnd then 8 else 54, pr = if atEnd then 54 else 12, gap = 8, align = "center" }, {
                    title(env, props.directory),
                    ui.spacer(),
                    ui.openInButton({ key = "files-open-in", path = path :: string }),
                }),
                ui.col({ w = "full", h = 1, fill = "border-base" }, {}),
            })
        end))
    end
    if path then
        table.insert(main, ui.codeView({ doc = env.Workspace:OpenDocument(path), grow = 1 }))
    else
        table.insert(main, empty(env, view))
    end
    local maxW = math.clamp(props.width * 0.6, 200, 480)
    return ui.panelSidebar({ id = "files", maxWidth = maxW, locked = launcher }, {
        if opened then sidebar(env, view, props, tree, path, bar.side) else ui.col({}, {}),
        ui.col({ key = "files-main", w = "full", h = "full" }, main),
    })
end

-- The narrow window's Files view: All files and the open files over the tree or the file.
function panel.narrow(env: Env, props: PanelProps): ui.Node
    return ui.fileTree({ tree = env.Workspace:OpenTree(props.directory), narrow = true, width = props.width, grow = 1 })
end

return panel
