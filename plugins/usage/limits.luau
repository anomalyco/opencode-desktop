--!strict
-- The usage limits the server reports (`GET /api/experimental/usage/limits`, Desktop's
-- `UsageLimits.Info`), and what the Usage dialog makes of them (`usage/limits.ts`): budgets
-- ordered by what stops the member first, their notes, the callout, the action and the footer.

local limits = {}

export type Workspace = {
    name: string?,
    usageUrl: string,
}

export type Meter = {
    percent: number,
    limited: boolean,
    resetsAt: number,
}

export type Go = {
    rolling: Meter,
    weekly: Meter,
    monthly: Meter,
}

export type BudgetModel = {
    provider: string,
    model: string,
    name: string,
    spent: number?,
}

export type Budget = {
    id: string,
    limit: number,
    spent: number,
    eachMember: boolean,
    perModel: boolean,
    coversAll: boolean,
    models: { BudgetModel },
    vendors: { string },
    inputPriceAtLeast: number?,
    picked: boolean,
    shared: boolean,
    resetsAt: number,
}

export type Info = {
    workspace: Workspace,
    go: Go?,
    budgets: { Budget },
}

export type CurrentModel = {
    provider: string,
    model: string,
    name: string,
}

export type Label =
    { kind: "all" }
    | { kind: "vendors", vendors: { string }, each: boolean, price: number? }
    | { kind: "price", price: number, each: boolean }
    | { kind: "models", names: { string }, each: boolean }
    | { kind: "each" }

export type Note =
    { kind: "runs-out-first" }
    | { kind: "paused" }
    | { kind: "includes-web-search" }
    | { kind: "paused-by", shared: boolean }
    | { kind: "current-model", name: string }

export type Sharing = "you" | "everyone" | "group"

export type Row = {
    label: Label,
    limit: number,
    perModel: boolean,
    sharing: Sharing,
    notes: { Note },
    reached: boolean,
    spent: number,
    left: number,
    fill: number,
}

export type Callout =
    { kind: "credits" }
    | { kind: "workspace", resetsAt: number }
    | { kind: "personal", resetsAt: number }

export type Suggestion = {
    name: string,
    left: number,
}

export type Action =
    { kind: "other-models", vendors: { string } }
    | { kind: "other-models-each", vendors: { string }, limit: number, suggestion: Suggestion? }

export type Footer =
    { kind: "resets", at: number }
    | { kind: "budgets-reset", at: number }
    | { kind: "credits-only" }

export type GoRow = {
    window: "rolling" | "weekly" | "monthly",
    meter: Meter,
}

export type UsageView = {
    workspace: string?,
    callout: Callout?,
    go: { GoRow }?,
    budgets: { Row },
    dimmed: boolean,
    action: Action?,
    footer: Footer,
}

local function parseMeter(m: any): Meter?
    if type(m) ~= "table" then
        return nil
    end
    if type(m.percent) ~= "number" or type(m.limited) ~= "boolean" or type(m.resetsAt) ~= "number" then
        return nil
    end
    return {
        percent = m.percent,
        limited = m.limited,
        resetsAt = math.floor(m.resetsAt),
    }
end

local function parseStrings(v: any): { string }
    local out: { string } = {}
    if type(v) ~= "table" then
        return out
    end
    for _, item in v do
        if type(item) == "string" then
            table.insert(out, item)
        end
    end
    return out
end

-- Parses the `data` payload of `/api/experimental/usage/limits`; nil for anything else.
function limits.parseInfo(raw: any): Info?
    if type(raw) ~= "table" then
        return nil
    end
    local v = if raw.workspace == nil and type(raw.data) == "table" then raw.data else raw
    local w = v.workspace
    if type(w) ~= "table" or type(w.usageURL) ~= "string" then
        return nil
    end
    if type(v.budgets) ~= "table" then
        return nil
    end

    local go: Go? = nil
    if type(v.go) == "table" then
        local rolling = parseMeter(v.go.rolling)
        local weekly = parseMeter(v.go.weekly)
        local monthly = parseMeter(v.go.monthly)
        if rolling and weekly and monthly then
            go = { rolling = rolling, weekly = weekly, monthly = monthly }
        end
    end

    local budgets: { Budget } = {}
    for _, b in v.budgets do
        if
            type(b) == "table"
            and type(b.id) == "string"
            and type(b.limit) == "number"
            and type(b.spent) == "number"
            and type(b.resetsAt) == "number"
        then
            local models: { BudgetModel } = {}
            if type(b.models) == "table" then
                for _, m in b.models do
                    if
                        type(m) == "table"
                        and type(m.providerID) == "string"
                        and type(m.modelID) == "string"
                        and type(m.name) == "string"
                    then
                        table.insert(models, {
                            provider = m.providerID,
                            model = m.modelID,
                            name = m.name,
                            spent = if type(m.spent) == "number" then m.spent else nil,
                        })
                    end
                end
            end
            table.insert(budgets, {
                id = b.id,
                limit = b.limit,
                spent = b.spent,
                eachMember = b.scope == "each",
                perModel = b.per == "model",
                coversAll = b.coverage == "all",
                models = models,
                vendors = parseStrings(b.vendors),
                inputPriceAtLeast = if type(b.inputPriceAtLeast) == "number" then b.inputPriceAtLeast else nil,
                picked = b.picked == true,
                shared = b.shared == true,
                resetsAt = math.floor(b.resetsAt),
            })
        end
    end

    return {
        workspace = {
            name = if type(w.name) == "string" then w.name else nil,
            usageUrl = w.usageURL,
        },
        go = go,
        budgets = budgets,
    }
end

-- Whether an HTTP 2xx server response explicitly returned `null` or `{"data": null}` (signed out).
function limits.isSignedOutResponse(res: ServerResponse): boolean
    if not res.ok then
        return false
    end
    local text = string.match(res.text, "^%s*(.-)%s*$") or ""
    if text == "null" then
        return true
    end
    if type(res.json) == "table" and res.json.workspace == nil and res.json.data == nil then
        return string.match(text, '"data"%s*:%s*null') ~= nil
    end
    return false
end

type Spend = {
    current: string?,
    spent: number,
    reached: boolean,
    left: number,
}

local function budgetSpend(b: Budget, model: CurrentModel?): Spend
    local matchedModel: CurrentModel? = nil
    local matchedEntry: BudgetModel? = nil
    if b.perModel and model ~= nil then
        for _, e in b.models do
            if e.provider == model.provider and e.model == model.model then
                matchedModel = model
                matchedEntry = e
                break
            end
        end
    end
    local spent = if matchedEntry ~= nil and matchedEntry.spent ~= nil then matchedEntry.spent else b.spent
    return {
        current = if matchedModel ~= nil then matchedModel.name else nil,
        spent = spent,
        reached = spent >= b.limit,
        left = math.max(0, b.limit - spent),
    }
end

local function budgetLabel(b: Budget): Label
    local each = b.perModel
    if b.coversAll then
        return { kind = "all" }
    end
    if #b.vendors > 0 then
        return { kind = "vendors", vendors = b.vendors, each = each, price = b.inputPriceAtLeast }
    end
    if b.inputPriceAtLeast ~= nil then
        return { kind = "price", price = b.inputPriceAtLeast, each = each }
    end
    if b.picked then
        local names: { string } = {}
        for _, m in b.models do
            table.insert(names, m.name)
        end
        return { kind = "models", names = names, each = each }
    end
    return { kind = "each" }
end

local function budgetAction(b: Budget): Action
    if not b.perModel then
        return { kind = "other-models", vendors = b.vendors }
    end
    type RoomEntry = { index: number, name: string, left: number }
    local room: { RoomEntry } = {}
    for index, m in b.models do
        local left = math.max(0, b.limit - (m.spent or 0))
        if left > 0 then
            table.insert(room, { index = index, name = m.name, left = left })
        end
    end
    table.sort(room, function(a, c)
        if a.left ~= c.left then
            return a.left > c.left
        end
        return a.index < c.index
    end)
    local top = room[1]
    return {
        kind = "other-models-each",
        vendors = b.vendors,
        limit = b.limit,
        suggestion = if top then { name = top.name, left = top.left } else nil,
    }
end

-- `modelUsage` (anomalyco/opencode#64409): the tightest applicable budget wins.
function limits.modelLeft(info: Info, provider: string, model: string, served: boolean): number?
    local best: number? = nil
    for _, b in info.budgets do
        local spent: number? = nil
        if b.coversAll then
            if served then
                spent = b.spent
            end
        else
            for _, e in b.models do
                if e.provider == provider and e.model == model then
                    spent = if b.perModel then (e.spent or 0) else b.spent
                    break
                end
            end
        end
        if spent ~= nil then
            local left = math.max(0, b.limit - spent)
            if best == nil or left < best then
                best = left
            end
        end
    end
    return best
end

-- `usageView`: what the dialog shows for `info`; `credits` marks a workspace out of credits.
function limits.view(info: Info, model: CurrentModel?, credits: boolean): UsageView
    type Ranked = { index: number, budget: Budget, spend: Spend }
    local ranked: { Ranked } = {}
    for index, b in info.budgets do
        table.insert(ranked, { index = index, budget = b, spend = budgetSpend(b, model) })
    end
    -- Stable ascending sort by remaining budget (`total_cmp` in Rust).
    table.sort(ranked, function(a, b)
        if a.spend.left ~= b.spend.left then
            return a.spend.left < b.spend.left
        end
        return a.index < b.index
    end)

    local blocking: Budget? = nil
    local anyReached = false
    local reachedModels: Budget? = nil
    for _, entry in ranked do
        if entry.spend.reached then
            anyReached = true
            if blocking == nil and entry.budget.coversAll then
                blocking = entry.budget
            end
            if reachedModels == nil and not entry.budget.coversAll then
                reachedModels = entry.budget
            end
        end
    end

    local callout: Callout? = nil
    if credits then
        callout = { kind = "credits" }
    elseif blocking ~= nil then
        if blocking.shared then
            callout = { kind = "workspace", resetsAt = blocking.resetsAt }
        else
            callout = { kind = "personal", resetsAt = blocking.resetsAt }
        end
    end

    local resetsAt = 0
    for i, b in info.budgets do
        if i == 1 or b.resetsAt < resetsAt then
            resetsAt = b.resetsAt
        end
    end

    local count = #ranked
    local rows: { Row } = {}
    for idx, entry in ranked do
        local b = entry.budget
        local s = entry.spend
        local notes: { Note } = {}
        if idx == 1 and callout == nil and not anyReached and count > 1 then
            table.insert(notes, { kind = "runs-out-first" })
        end
        if s.reached and b.coversAll then
            table.insert(notes, { kind = "includes-web-search" })
        end
        -- A budget per model names the model it paused instead.
        if s.reached and not b.coversAll and not b.perModel then
            table.insert(notes, { kind = "paused" })
        end
        if not s.reached and blocking ~= nil then
            table.insert(notes, { kind = "paused-by", shared = blocking.shared })
        end
        if s.current ~= nil then
            table.insert(notes, { kind = "current-model", name = s.current })
        end
        local sharing: Sharing = if b.eachMember then "you" elseif b.shared then "everyone" else "group"
        table.insert(rows, {
            label = budgetLabel(b),
            limit = b.limit,
            perModel = b.perModel,
            sharing = sharing,
            notes = notes,
            reached = s.reached,
            spent = s.spent,
            left = s.left,
            fill = if b.limit > 0 then math.min(1, s.spent / b.limit) else 1,
        })
    end

    local goRows: { GoRow }? = nil
    if info.go ~= nil then
        goRows = {
            { window = "rolling", meter = info.go.rolling },
            { window = "weekly", meter = info.go.weekly },
            { window = "monthly", meter = info.go.monthly },
        }
    end

    local footer: Footer
    if #info.budgets == 0 then
        footer = { kind = "credits-only" }
    elseif credits then
        footer = { kind = "budgets-reset", at = resetsAt }
    else
        footer = { kind = "resets", at = resetsAt }
    end

    return {
        workspace = info.workspace.name,
        callout = callout,
        go = goRows,
        budgets = rows,
        dimmed = callout ~= nil,
        action = if callout == nil and reachedModels ~= nil then budgetAction(reachedModels) else nil,
        footer = footer,
    }
end

return limits
