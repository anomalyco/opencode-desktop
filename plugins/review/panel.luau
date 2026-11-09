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

export type Env = {
    I18n: I18nService,
    Diff: DiffService,
    Layout: LayoutService,
    Menu: MenuService,
    Theme: ThemeService,
    -- One document per session, opened once.
    docs: { [string]: DiffDocument },
    -- The sidebar's filter.
    filter: Editor,
    style: StoreValue<{ diffStyle: DiffStyle }>,
    expand: StoreValue<{ expandMode: ExpandMode }>,
    -- Each session's source.
    source: SessionStoreValue<{ mode: DiffSource? }>,
}

-- Session `sid`'s changes, read from its source.
function panel.doc(env: Env, sid: string): DiffDocument
    local doc = env.docs[sid]
    if doc == nil then
        doc = env.Diff:Open({ sessionId = sid, source = env.source:Get(sid).mode or "git" })
        env.docs[sid] = doc
    end
    return doc
end

-- Compares `mode` in session `sid`, and remembers it.
-- Desktop's review style, kept, and the diffs drawn in it.
function panel.setStyle(env: Env, style: DiffStyle)
    env.style:Set({ diffStyle = style })
    env.Diff:SetMode(style)
end

function panel.setSource(env: Env, sid: string, mode: DiffSource)
    env.source:Set(sid, { mode = mode })
    panel.doc(env, sid):SetSource(mode)
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

-- The source picker (`ReviewTitle`: select-v2 inline, 24 tall, 8 4 in, 4 round): the source in
-- 13 / 16 at 530 and select-v2's 16 pt chevron, which lists the sources this folder has; with
-- one source, just its name (`ps-2 pe-1`).
function panel.title(env: Env, view: View, sid: string, key: string): ui.Node
    local doc = panel.doc(env, sid)
    local sources = doc:Sources()
    local current = doc:Source()
    local label = ui.text(sourceLabel(env, current), { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text", fadeOut = true })
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

-- A folder outside version control (`SessionReviewEmptyNoGitV2`).
local function noGit(env: Env): ui.Node
    return ui.col({ key = "review-no-git", w = "full", grow = 1, align = "center", justify = "center", gap = 16, pad = 24, pb = 160 }, {
        ui.icon("file:.gitignore", { size = 20, color = "icon-weak" }),
        ui.text(env.I18n:T("ui.sessionReviewV2.empty.noGit.title"), { size = 13, weight = "medium", lineHeight = 16, tracking = -0.04, color = "text", textAlign = "center" }),
        ui.text(env.I18n:T("ui.sessionReviewV2.empty.noGit.description"), { size = 13, weight = "regular", lineHeight = 20, tracking = -0.04, color = "text-muted", maxW = 360, textAlign = "center", mb = 4 }),
    })
end

-- The sidebar: its header (source, totals, Open in without changes), the filter and the tree.
local function sidebar(env: Env, view: View, sid: string, doc: DiffDocument, files: { DiffFile }, active: string?): ui.Node
    local totals = doc:Totals()
    local status = doc:Status()
    local query = env.filter:Text()
    local headerEnd: { ui.Node } = { counts(totals.added, totals.removed) }
    if #files == 0 then
        table.insert(headerEnd, ui.openInButton({ key = "review-open-in", path = "" }))
    end
    -- `session-review-v2-sidebar-header`: 48 tall, 46 in past the toggle, 16 from the end.
    local header = ui.row({ key = "review-sidebar-header", w = "full", h = 48, pl = 46, pr = 16, gap = 8, align = "center" }, {
        panel.title(env, view, sid, "review-mode-btn"),
        ui.spacer(),
        ui.row({ gap = 8, align = "center" }, headerEnd),
    })
    local filter = ui.field({ key = "review-filter", editor = env.filter, variant = "filter", placeholder = env.I18n:T("ui.sessionReviewV2.filterFiles") })
    local list = if status == "loading"
        then ui.col({ key = "review-tree-loading", w = "full", grow = 1, px = 10, pt = 12 }, {
            ui.text(env.I18n:T("ui.sessionReview.image.loading"), { size = 12, lineHeight = 16, color = "text-muted" }),
        })
        else ui.fileTree({
            tree = doc:Tree(),
            filter = query,
            kinds = kinds.marks(kinds.of(files)),
            selectedPath = active,
            pt = 4,
            onSelect = view:Handler(function(event)
                if event.path then
                    doc:Focus(event.path)
                end
            end),
        })
    return ui.col({ w = "full", h = "full" }, { header, filter, list })
end

-- The toolbar: with the sidebar shut its title, totals and the file's place; the file arrows;
-- then Open in, the line folding and the layout, 12 apart.
local function toolbar(env: Env, view: View, sid: string, doc: DiffDocument, order: { string }, active: string?, opened: boolean): ui.Node
    local start: { ui.Node } = {}
    local index = 0
    for i, path in order do
        if path == active then
            index = i
        end
    end
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
            doc:Focus(order[(at - 1 + by) % #order + 1])
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
        value = env.expand:Get().expandMode,
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
        value = env.style:Get().diffStyle,
        items = {
            { id = "unified", icon = "review-unified", label = env.I18n:T("ui.sessionReviewV2.unifiedDiff"), key = "review-style-unified" },
            { id = "split", icon = "review-split", label = env.I18n:T("ui.sessionReviewV2.splitDiff"), key = "review-style-split" },
        },
        onChange = view:Handler(function(event)
            panel.setStyle(env, (event.value or "split") :: DiffStyle)
        end),
    }))
    return ui.col({ w = "full" }, {
        ui.row({ key = "review-toolbar", w = "full", h = 48, pl = if opened then 8 else 46, pr = 12, gap = 8, align = "center", fill = "bg-base" }, {
            ui.row({ key = "review-toolbar-start", align = "center", shrink = true, minW = 0 }, start),
            ui.spacer(),
            ui.row({ key = "review-toolbar-segments", gap = 12, align = "center" }, segments),
        }),
        ui.divider({ color = "border-base" }),
    })
end

-- The Review tab in the side region.
function panel.wide(env: Env, view: View, props: PanelProps): ui.Node
    local sid = props.sessionId
    local doc = panel.doc(env, sid)
    local status = doc:Status()
    local files = doc:Files()
    local order = kinds.sorted(kinds.paths(files))
    local active = doc:Active()
    local bar = env.Layout:PanelSidebar()
    local main: { ui.Node } = {}
    if #files > 0 or not bar.opened then
        table.insert(main, toolbar(env, view, sid, doc, order, active, bar.opened))
    end
    if status == "loading" and #files == 0 then
        table.insert(main, loading(env))
    elseif status == "no-vcs" then
        table.insert(main, noGit(env))
    elseif #files == 0 then
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
        sidebar(env, view, sid, doc, files, active),
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
