--!strict
-- The Review tab (panel.tsx, SessionReviewV2): the file sidebar with the change source, the
-- totals, a filter and the change tree; then the toolbar (with the sidebar shut, the source, the
-- totals and the file's place among them), the file arrows, Open in, Show all lines / Hide
-- unchanged lines and Unified / Split; and the preview of one file: the one picked, else the
-- newest change. With no changes, Desktop's empty view.

local ui = require("@opencode/ui")
local kinds = require("./kinds")

local panel = {}

export type DiffStyle = "split" | "unified"
export type ExpandMode = "expand" | "collapse"

export type CachedDiff = {
    version: number,
    files: { DiffFile },
    kinds: { [string]: ChangeKind },
    marks: { [string]: ChangeKind },
    order: { string },
}

export type CachedKinds = {
    treeVersion: number,
    kinds: { [string]: ChangeKind },
    marks: { [string]: ChangeKind },
    order: { string },
}

export type Env = {
    I18n: I18nService,
    Diff: DiffService,
    Layout: LayoutService,
    Menu: MenuService,
    Theme: ThemeService,
    Sessions: SessionsService,
    menuTrigger: boolean,
    -- One document per session, opened once.
    docs: { [string]: DiffDocument },
    -- The sessions whose document opened before the server listed them.
    unlisted: { [string]: boolean },
    cache: { [string]: CachedDiff }?,
    kindsCache: { [string]: CachedKinds }?,
    -- The sidebar's filter.
    filter: Editor,
    style: StoreValue<{ diffStyle: DiffStyle }>,
    expand: StoreValue<{ expandMode: ExpandMode }>,
    -- Each session's source and the file its preview shows (Desktop's session store, `model.ts`).
    source: SessionStoreValue<{ mode: DiffSource?, file: string? }>,
}

-- Session `sid`'s changes, read from its source.
function panel.doc(env: Env, sid: string): DiffDocument
    local doc = env.docs[sid]
    if doc == nil then
        local stored = env.source:Get(sid)
        doc = env.Diff:Open({ sessionId = sid, source = stored.mode or "git" })
        env.docs[sid] = doc
        -- The file picked last shows again while the changes still list it (`activeFile`).
        if stored.file then
            doc:Focus(stored.file)
        end
        -- A session the server hasn't listed yet (a subagent's page at launch) has no folder, and
        -- its choice is kept under its folder: that first read found none.
        if env.Sessions:Get(sid) == nil then
            env.unlisted[sid] = true
        end
    elseif env.unlisted[sid] and env.Sessions:Get(sid) ~= nil then
        -- Listed now: the source and the file it chose apply over the defaults it opened with
        -- (Desktop waits for the stored state before it shows or asks for anything, `model.ts`).
        env.unlisted[sid] = nil
        local stored = env.source:Get(sid)
        if stored.mode and stored.mode ~= doc:Source() then
            doc:SetSource(stored.mode)
        end
        if stored.file then
            doc:Focus(stored.file)
        end
    end
    return doc
end

-- Memoizes `doc:Files()`, `kinds.of(...)`, `kinds.marks(...)` and `kinds.sorted(...)` per
-- `doc:Version()`, so file reloads and multi-consumer reads do not re-marshal or re-sort paths.
function panel.cached(env: Env, sid: string): CachedDiff
    local doc = panel.doc(env, sid)
    local v = doc:Version()
    local store = env.cache
    if store == nil then
        store = {}
        env.cache = store
    end
    local cur = (store :: { [string]: CachedDiff })[sid]
    if cur ~= nil and cur.version == v then
        return cur
    end
    local files = doc:Files()
    local k = kinds.of(files)
    local built: CachedDiff = {
        version = v,
        files = files,
        kinds = k,
        marks = kinds.marks(k),
        order = kinds.sorted(kinds.paths(files)),
    }
    ;(store :: { [string]: CachedDiff })[sid] = built
    return built
end

-- Memoizes `kinds`, `marks` and `order` per `doc:Tree():Version()` (`Dep::DiffTree`), so line-only
-- edits inside an already-modified file never re-marshal `doc:Files()` or dirty `review.changes`.
function panel.cachedTree(env: Env, sid: string): CachedKinds
    local doc = panel.doc(env, sid)
    local tv = doc:Tree():Version()
    local store = env.kindsCache
    if store == nil then
        store = {}
        env.kindsCache = store
    end
    local cur = (store :: { [string]: CachedKinds })[sid]
    if cur ~= nil and cur.treeVersion == tv then
        return cur
    end
    local files = doc:Files()
    local k = kinds.of(files)
    local built: CachedKinds = {
        treeVersion = tv,
        kinds = k,
        marks = kinds.marks(k),
        order = kinds.sorted(kinds.paths(files)),
    }
    ;(store :: { [string]: CachedKinds })[sid] = built
    return built
end

function panel.cachedKinds(env: Env, sid: string): { [string]: ChangeKind }
    return panel.cachedTree(env, sid).kinds
end

-- Compares `mode` in session `sid`, and remembers it.
-- Desktop's review style, kept, and the diffs drawn in it.
function panel.setStyle(env: Env, style: DiffStyle)
    env.style:Set({ diffStyle = style })
    env.Diff:SetMode(style)
end

function panel.setSource(env: Env, sid: string, mode: DiffSource)
    env.source:Set(sid, { mode = mode, file = env.source:Get(sid).file })
    panel.doc(env, sid):SetSource(mode)
end

-- Shows `path` in session `sid`'s preview, and remembers it (`focusFile`).
function panel.focus(env: Env, sid: string, path: string)
    env.source:Set(sid, { mode = env.source:Get(sid).mode, file = path })
    panel.doc(env, sid):Focus(path)
end

local TEXT = { size = 13, weight = "regular" :: FontWeight, lineHeight = 16, tracking = -0.04 }

local function sourceLabel(env: Env, mode: DiffSource): string
    if mode == "git" then
        return env.I18n:T("ui.sessionReview.title.git")
    elseif mode == "branch" then
        return env.I18n:T("ui.sessionReview.title.branch")
    end
    return env.I18n:T("ui.sessionReview.title.lastTurn")
end

-- Whether session `sid`'s saved source can't be read yet: the server hasn't listed the session,
-- and its choice is kept under its folder. Desktop's `canReview` (`model.ts`: "the mode picker
-- waits for the stored mode").
function panel.held(env: Env, sid: string): boolean
    -- `panel.doc` asks while the session is unlisted, and not again once it is listed.
    panel.doc(env, sid)
    return env.unlisted[sid] == true
end

-- The source picker (`ReviewTitle`: select-v2 inline, 24 tall, 8 4 in, 4 round): the source in
-- 13 / 16 at 530 in select-v2's text-text-base and its 16 pt chevron, which lists the sources
-- this folder has; with one source, just its name (`ps-2 pe-1`).
function panel.title(env: Env, view: View, sid: string, key: string): ui.Node
    local doc = panel.doc(env, sid)
    -- No source shows before the saved one can be read (`ReviewTitle`, `parts.tsx`): naming the
    -- default here would name a source the session may not be in.
    if panel.held(env, sid) then
        return ui.row({ key = `{key}-held`, h = 24 }, {})
    end
    local sources = doc:Sources()
    local current = doc:Source()
    local label = ui.text(sourceLabel(env, current), { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text-chrome", fadeOut = true })
    if #sources <= 1 then
        return ui.row({ key = `{key}-static`, h = 24, pl = 8, pr = 4, align = "center" }, { label })
    end
    local chevron = ui.icon("select-chevron", { size = 16, color = "icon-weak" })
    return ui.row({
        key = key,
        role = "button",
        label = env.I18n:T("rune.review.scope"),
        h = 24,
        py = 4,
        pl = 8,
        pr = 4,
        gap = 4,
        align = "center",
        radius = 4,
        hover = "surface-hover",
        press = "surface-press",
        hoverTransition = "instant",
        shrink = true,
        minW = 0,
        menuTrigger = env.menuTrigger,
        onPress = view:Handler(function()
            local items: { MenuEntry } = {}
            for _, mode in sources do
                table.insert(items, { id = mode, title = sourceLabel(env, mode), checked = mode == current })
            end
            local picked = env.Menu:OpenAsync({ anchorKey = key, items = items, minWidth = 164 })
            if picked ~= nil and picked ~= current then
                panel.setSource(env, sid, picked :: DiffSource)
            end
        end),
    }, { label, chevron })
end

-- `DiffChanges` compact: `+N` and `-M`, 8 apart, in 11 / 11 at 440 with 0.05 tracking; nothing
-- without changes.
local function counts(added: number, removed: number): ui.Node
    local parts: { ui.Node } = {}
    local style = function(color: Token): ui.TextProps
        return { size = 11, weight = "regular", lineHeight = 11, tracking = 0.05, color = color, noWrap = true }
    end
    if added + removed > 0 then
        table.insert(parts, ui.text(`+{added}`, style("status-success")))
        table.insert(parts, ui.text(`-{removed}`, style("status-danger")))
    end
    return ui.row({ key = "stats", gap = 8, align = "center" }, parts)
end

-- A file arrow: 36 × 28, 6 round, its 16 pt arrow; quiet and inert without changes.
local function arrow(env: Env, view: View, key: string, icon: string, help: string, go: (() -> ())?): ui.Node
    return ui.row({
        key = key,
        w = 36,
        h = 28,
        radius = 6,
        center = true,
        role = "button",
        label = help,
        help = help,
        hover = if go then "surface-hover" else nil,
        hoverTransition = "instant",
        opacity = if go then nil else 0.35,
        disabled = go == nil,
        onPress = if go then view:Handler(function()
            go()
        end) else nil,
    }, { ui.icon(icon, { size = 16, color = "icon-base" }) })
end

-- Desktop's empty Review (`SessionReviewEmptyChangesV2`): the review icon, "No file changes yet"
-- and "Project changes will appear here", 160 above the foot.
local function emptyChanges(env: Env): ui.Node
    return ui.col({ key = "review-empty", w = "full", grow = 1, align = "center", justify = "center", gap = 8, pad = 24, pb = 160 }, {
        ui.icon("review", { size = 20, color = "icon-weak", mb = 8 }),
        ui.text(env.I18n:T("ui.sessionReviewV2.empty.changes.title"), { size = 13, weight = "medium", lineHeight = 13, tracking = -0.04, color = "text", textAlign = "center" }),
        ui.text(env.I18n:T("ui.sessionReviewV2.empty.changes.description"), { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-muted", maxW = 282, textAlign = "center" }),
    })
end

-- Before the changes load (`ReviewPanelEmpty`'s loading).
local function loading(env: Env): ui.Node
    return ui.col({ key = "review-loading", w = "full", grow = 1, px = 24, py = 16 }, {
        ui.text(env.I18n:T("loadingChanges"), { size = 14, lineHeight = 20, color = "text-weak" }),
    })
end

-- Beyond Desktop, which says "No file changes yet" when the read fails: what failed, why, and
-- Try again, where the empty view stands.
local function failed(env: Env, view: View, doc: DiffDocument, why: string): ui.Node
    return ui.col({ key = "review-failed", w = "full", grow = 1, align = "center", justify = "center", gap = 8, pad = 24, pb = 160 }, {
        ui.text(env.I18n:T("rune.review.readFailed"), { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text", textAlign = "center" }),
        ui.text(why, { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-muted", maxW = 360, textAlign = "center", mb = 4 }),
        ui.row({
            key = "review-retry",
            h = 28,
            px = 10,
            radius = 6,
            center = true,
            role = "button",
            label = env.I18n:T("common.retry"),
            fill = "surface-raised",
            hover = "surface-hover",
            press = "surface-press",
            hoverTransition = "instant",
            onPress = view:Handler(function()
                doc:Retry()
            end),
        }, { ui.text(env.I18n:T("common.retry"), { size = 13, weight = "medium", lineHeight = 20, color = "text" }) }),
    })
end

-- A folder outside version control (`SessionReviewEmptyNoGitV2`).
local function noGit(env: Env): ui.Node
    return ui.col({ key = "review-no-git", w = "full", grow = 1, align = "center", justify = "center", gap = 16, pad = 24, pb = 160 }, {
        ui.icon("file:.gitignore", { size = 20, color = "icon-weak" }),
        ui.text(env.I18n:T("ui.sessionReviewV2.empty.noGit.title"), { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text", textAlign = "center" }),
        ui.text(env.I18n:T("ui.sessionReviewV2.empty.noGit.description"), { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-muted", maxW = 360, textAlign = "center", mb = 4 }),
    })
end

-- The sidebar: its header (source, totals, Open in without changes), the filter and the tree.
local function sidebar(env: Env, view: View, sid: string, doc: DiffDocument, marks: { [string]: ChangeKind }, active: string?, treeVer: number, side: PanelSidebarSide): ui.Node
    local totals = doc:Totals()
    local status = doc:Status()
    local query = env.filter:Text()
    local look = `{env.I18n:Locale().id}:{tostring(view.dark)}`
    local titleNode = view:Memo("review-title-sidebar-slot", `{sid}:{doc:Source()}:{#doc:Sources()}:{tostring(panel.held(env, sid))}:{look}`, function()
        return ui.row({ align = "center", shrink = true, minW = 0 }, {
            panel.title(env, view, sid, "review-mode-btn"),
        })
    end)
    local headerEnd: { ui.Node } = {}
    if totals.added + totals.removed > 0 then
        table.insert(headerEnd, counts(totals.added, totals.removed))
    end
    if totals.files == 0 then
        table.insert(headerEnd, ui.openInButton({ key = "review-open-in", path = "" }))
    end
    local headerItems: { ui.Node } = { titleNode }
    if #headerEnd > 0 then
        table.insert(headerItems, ui.row({ gap = 8, align = "center" }, headerEnd))
    end
    -- `session-review-v2-sidebar-header`: 48 tall, 46 in past the toggle, 16 from the end; with the
    -- sidebar on the end edge the toggle is at its end (`data-sidebar-side="end"`: `8px 46px`).
    local atEnd = side == "end"
    local header = ui.row({ key = "review-sidebar-header", w = "full", h = 48, pl = if atEnd then 8 else 46, pr = if atEnd then 46 else 16, gap = 8, align = "center", justify = "between" }, headerItems)
    local filter = view:Memo("review-filter-slot", `{env.filter:Version()}:{look}`, function()
        return ui.col({ w = "full" }, {
            ui.field({ key = "review-filter", editor = env.filter, variant = "filter", placeholder = env.I18n:T("ui.sessionReviewV2.filterFiles") }),
        })
    end)
    local listVer = if query == "" then treeVer else doc:Version()
    local list = view:Memo("review-tree-slot", `{sid}:{status}:{totals.files}:{query}:{active or ""}:{listVer}:{look}`, function()
        -- `session-review-v2-sidebar-viewport` (`padding: 4px 8px 12px`) + `px-2 py-2 text-12-regular text-text-weak`
        -- (`panel.tsx:448-462`; `.text-12-regular` resolves via `utilities.css:21-28` + `theme.css:8,16` to `--font-size-small: 13px`, `--line-height-large: 150%` = `19.5px`).
        local function sidebarMessage(key: string, words: string, isStatus: boolean): ui.Node
            return ui.col({ key = key, role = if isStatus then "status" else nil, w = "full", grow = 1, px = 16, pt = 12, pb = 20 }, {
                ui.text(words, { size = 13, weight = "regular", lineHeight = 19.5, color = "text-weak", wrap = true }),
            })
        end
        -- Keep the loaded file tree during background refreshes and transient disconnects/errors,
        -- matching the main pane (`panel.luau:437-444`) and official `review/machine.ts:150-153, 171-172, 220-225, 238-240`.
        if totals.files == 0 then
            if status == "loading" then
                return sidebarMessage("review-tree-loading", env.I18n:T("ui.sessionReview.image.loading"), false)
            end
            if status == "offline" then
                return sidebarMessage("review-tree-offline", env.I18n:T("offline"), false)
            end
            if status == "error" then
                return sidebarMessage("review-tree-error", env.I18n:T("load.failed"), false)
            end
            return sidebarMessage("review-tree-empty", env.I18n:T("sidebar.noChanges"), true)
        end
        return ui.col({ w = "full", grow = 1, minH = 0 }, {
            ui.fileTree({
                tree = doc:Tree(),
                filter = query,
                kinds = marks,
                selectedPath = active,
                pt = 4,
                grow = 1,
                onSelect = view:Handler(function(event)
                    if event.path then
                        panel.focus(env, sid, event.path)
                    end
                end),
            }),
        })
    end)
    return ui.col({ w = "full", h = "full" }, { header, filter, list })
end

-- The toolbar: with the sidebar shut its title, totals and the file's place; the file arrows;
-- then Open in, the line folding and the layout, 12 apart.
local function toolbar(env: Env, view: View, sid: string, doc: DiffDocument, order: { string }, active: string?, opened: boolean, treeVer: number, side: PanelSidebarSide): ui.Node
    local index = 0
    for i, path in order do
        if path == active then
            index = i
        end
    end
    local expandMode = env.expand:Get().expandMode
    local diffStyle = env.style:Get().diffStyle
    local look = `{env.I18n:Locale().id}:{tostring(view.dark)}`
    local shutMeta = ""
    if not opened then
        local totals = doc:Totals()
        shutMeta = `{doc:Source()}:{#doc:Sources()}:{tostring(panel.held(env, sid))}:{totals.added}:{totals.removed}`
    end
    return view:Memo("review-toolbar-slot", `{sid}:{active or ""}:{#order}:{index}:{treeVer}:{tostring(opened)}:{side}:{expandMode}:{diffStyle}:{shutMeta}:{look}`, function()
        local start: { ui.Node } = {}
        if not opened then
            local totals = doc:Totals()
            local meta: { ui.Node } = { panel.title(env, view, sid, "review-mode-btn"), counts(totals.added, totals.removed) }
            if #order > 0 then
                table.insert(meta, ui.text(`{math.max(index, 1)}/{#order}`, { size = 11, lineHeight = 13, tracking = 0.05, tabular = true, color = "text-faint", noWrap = true }))
            end
            table.insert(start, ui.row({ key = "review-collapsed-meta", gap = 8, pr = 8, align = "center", shrink = true, minW = 0 }, meta))
        end
        local function step(by: number): (() -> ())?
            if #order == 0 then
                return nil
            end
            return function()
                local at = if index == 0 then 1 else index
                panel.focus(env, sid, order[(at - 1 + by) % #order + 1])
            end
        end
        table.insert(start, arrow(env, view, "review-prev-file", "review-arrow-left", `{env.I18n:T("ui.sessionReviewV2.previousFile")} \u{2190}`, step(-1)))
        table.insert(start, arrow(env, view, "review-next-file", "review-arrow-right", `{env.I18n:T("ui.sessionReviewV2.nextFile")} \u{2192}`, step(1)))
        local segments: { ui.Node } = {}
        if active then
            table.insert(segments, ui.openInButton({ key = "review-open-in", path = active }))
        end
        table.insert(segments, ui.segmented({
            key = "review-expand-seg",
            value = expandMode,
            items = {
                { id = "expand", icon = "review-expand-lines", label = env.I18n:T("ui.sessionReviewV2.showAllLines"), key = "review-expand-all" },
                { id = "collapse", icon = "review-collapse-lines", label = env.I18n:T("ui.sessionReviewV2.hideNonDiffLines"), key = "review-collapse-unchanged" },
            },
            onChange = view:Handler(function(event)
                env.expand:Set({ expandMode = (event.value or "collapse") :: ExpandMode })
            end),
        }))
        table.insert(segments, ui.segmented({
            key = "review-style-seg",
            value = diffStyle,
            items = {
                { id = "unified", icon = "review-unified", label = env.I18n:T("ui.sessionReviewV2.unifiedDiff"), key = "review-style-unified" },
                { id = "split", icon = "review-split", label = env.I18n:T("ui.sessionReviewV2.splitDiff"), key = "review-style-split" },
            },
            onChange = view:Handler(function(event)
                panel.setStyle(env, (event.value or "split") :: DiffStyle)
            end),
        }))
        -- 48 tall with its rule inside (`session-review-v2-toolbar`: `h-12`, border-box). Shut, it
        -- leaves the toggle 46: at its start, or at its end with the sidebar on the end edge.
        local shutEnd = not opened and side == "end"
        return ui.col({ w = "full", h = 48 }, {
            ui.row({ key = "review-toolbar", w = "full", grow = 1, minH = 0, pl = if opened or shutEnd then 8 else 46, pr = if shutEnd then 46 else 12, gap = 8, align = "center", fill = "bg-base" }, {
                ui.row({ key = "review-toolbar-start", align = "center", shrink = true, minW = 0 }, start),
                ui.spacer(),
                ui.row({ key = "review-toolbar-segments", gap = 12, align = "center" }, segments),
            }),
            ui.divider({ color = "border-base" }),
        })
    end)
end

-- The Review tab in the side region.
function panel.wide(env: Env, view: View, props: PanelProps): ui.Node
    local sid = props.sessionId
    local doc = panel.doc(env, sid)
    local status = doc:Status()
    local cur = panel.cachedTree(env, sid)
    local order = cur.order
    local active = doc:Active()
    local bar = env.Layout:PanelSidebar()
    local main: { ui.Node } = {}
    local hasRows = doc:Totals().files > 0
    if hasRows or not bar.opened then
        table.insert(main, toolbar(env, view, sid, doc, order, active, bar.opened, cur.treeVersion, bar.side))
    end
    if status == "loading" and #order == 0 then
        table.insert(main, loading(env))
    elseif (status == "error" or status == "offline") and #order == 0 then
        table.insert(main, failed(env, view, doc, doc:Error() or ""))
    elseif status == "no-vcs" then
        table.insert(main, noGit(env))
    elseif not hasRows then
        table.insert(main, emptyChanges(env))
    else
        table.insert(main, ui.diffView({
            diff = doc,
            mode = env.style:Get().diffStyle,
            expand = env.expand:Get().expandMode == "expand",
            file = active,
            grow = 1,
        }))
    end
    local maxW = math.clamp(props.width * 0.6, 200, 480)
    return ui.panelSidebar({ id = "review", maxWidth = maxW }, {
        if bar.opened then sidebar(env, view, sid, doc, cur.marks, active, cur.treeVersion, bar.side) else ui.col({}, {}),
        ui.col({ w = "full", h = "full" }, main),
    })
end

-- The narrow Changes view: every file in one list, its source picker at the header's start.
function panel.mobile(env: Env, view: View, props: PanelProps): ui.Node
    local sid = props.sessionId
    local doc = panel.doc(env, sid)
    return ui.diffView({ diff = doc, mode = env.style:Get().diffStyle, expand = env.expand:Get().expandMode == "expand", list = true, grow = 1 }, { panel.title(env, view, sid, "narrow-review-scope") })
end

return panel
