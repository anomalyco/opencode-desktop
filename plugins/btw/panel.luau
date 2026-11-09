--!strict
-- A side question's tab (`panel.tsx`): the question as a prompt with its agent, model and time,
-- then Working while it runs, the answer as a reply with Copy and its agent, model and duration,
-- or why it failed with Retry. Its label in the strip is the question beside the comment icon.

local ui = require("@opencode/ui")
local machine = require("./machine")

local panel = {}

-- `md:max-w-session`: the timeline's width.
local SESSION_W = 744
-- A prompt bubble's widest (`min(82%, 64ch)` at 14 pt Inter, as the timeline's).
local BUBBLE_MAX = 565

export type Env = {
    I18n: I18nService,
    Models: ModelsService,
    System: SystemService,
    state: machine.State,
    saved: (session: string) -> { machine.Saved },
    -- What each chat's Copy buttons copied: the text, so a changed text clears the mark.
    copied: { [string]: { question: string?, answer: string? } },
    retry: (session: string, id: string) -> (),
}

local function capitalize(s: string?): string
    if s == nil or s == "" then
        return ""
    end
    return s:sub(1, 1):upper() .. s:sub(2)
end

-- The model's name in the catalog (else its id), with a variant other than default after it.
local function modelLabel(env: Env, m: machine.ChatModel?): string
    if m == nil then
        return ""
    end
    local name = env.Models:Get(m.providerID, m.id).modelName or m.id
    if m.variant == nil or m.variant == "" or m.variant == "default" then
        return name
    end
    return env.I18n:T("modelVariant", { model = name, variant = capitalize(m.variant) })
end

function panel.label(env: Env, view: View, props: PanelTabLabelProps): ui.Node?
    local id = props.instance or ""
    local title = machine.tab(env.state, env.saved(props.sessionId), props.sessionId, id) or env.I18n:T("tab.title")
    -- A label of its own keeps the trigger's ink, selected or not (`flex items-center gap-1.5`).
    return view:Memo(`btw-label-{id}`, title, function()
        return ui.row({ shrink = true, minW = 0, gap = 6, align = "center" }, {
            ui.icon("comment", { size = 16, color = "text-muted" }),
            ui.text(
                title,
                {
                    size = 13,
                    weight = "regular",
                    lineHeight = 20,
                    color = "text-muted",
                    truncate = true,
                    shrink = true,
                    minW = 0,
                }
            ),
        })
    end)
end

-- Desktop's `IconButton size="normal" variant="ghost-muted"` with the copy glyph (a check once
-- copied) at 14, its tooltip above.
local function copyButton(key: string, copied: boolean, tip: string, onPress: ui.Handler): ui.Node
    return ui.row({
        key = key,
        role = "button",
        label = tip,
        help = tip,
        helpSide = "top",
        w = 24,
        h = 24,
        center = true,
        radius = 6,
        hover = "surface-hover",
        press = "surface-press",
        hoverTransition = "instant",
        onPress = onPress,
    }, { ui.icon(if copied then "check" else "outline-copy", { size = 14, color = "icon-weak" }) })
end

local META = {
    size = 12,
    weight = "regular" :: FontWeight,
    lineHeight = 18,
    tracking = 0.05,
    color = "text-faint" :: Color,
    noWrap = true,
}

local function meta(words: string, truncate: boolean?): ui.Node
    return ui.text(words, {
        size = META.size,
        weight = META.weight,
        lineHeight = META.lineHeight,
        tracking = META.tracking,
        color = META.color,
        noWrap = true,
        truncate = truncate,
        shrink = truncate,
        minW = if truncate then 0 else nil,
    })
end

-- The question (`user-message`): its bubble at the end, and under it, while hovered, its agent,
-- model and time with Copy message.
local function prompt(
    env: Env,
    view: View,
    id: string,
    info: machine.Info,
    agent: string,
    model: string,
    stamp: string
): ui.Node
    local I18n = env.I18n
    local marks = env.copied[id]
    local copied = marks ~= nil and marks.question == info.question
    local parts: { string } = {}
    for _, s in { agent, model } do
        if s ~= "" then
            table.insert(parts, s)
        end
    end
    local userMeta = table.concat(parts, "\u{a0}\u{b7}\u{a0}")
    local line: { ui.Node } = {}
    if userMeta ~= "" then
        table.insert(line, meta(userMeta, true))
    end
    if userMeta ~= "" and stamp ~= "" then
        table.insert(line, meta("\u{a0}\u{b7}\u{a0}"))
    end
    if stamp ~= "" then
        table.insert(line, meta(stamp))
    end
    local tip = I18n:T(if copied then "copied" else "copyMessage")
    local question = info.question
    return ui.col({ key = `btw-prompt-{id}`, w = "full", align = "end", hoverGroup = true }, {
        ui.row({ w = "full" }, {
            ui.spacer(),
            ui.row(
                {
                    key = "btw-question",
                    maxW = BUBBLE_MAX,
                    shrink = true,
                    px = 12,
                    py = 8,
                    radius = 10,
                    fill = "surface-2",
                },
                {
                    ui.text(
                        question,
                        {
                            size = 14,
                            weight = "regular",
                            lineHeight = 21,
                            color = "text-chrome",
                            wrap = true,
                            selectable = true,
                        }
                    ),
                }
            ),
        }),
        ui.row(
            { key = "btw-question-footer", w = "full", h = 20, mt = 8, gap = 8, align = "center", revealOnHover = true },
            {
                ui.spacer(),
                ui.row({ key = "btw-question-meta", minW = 0, shrink = true, align = "center" }, line),
                copyButton(
                    "btw-copy-question",
                    copied,
                    tip,
                    view:Handler(function()
                        env.System:CopyText(question)
                        local m: { question: string?, answer: string? } = env.copied[id] or {}
                        m.question = question
                        env.copied[id] = m
                    end)
                ),
            }
        ),
    })
end

-- The answer (`text-part`): Markdown at 14, and under it, while hovered, Copy answer with its
-- agent, model and duration.
local function answered(
    env: Env,
    view: View,
    id: string,
    answer: string,
    agent: string,
    model: string,
    duration: number?
): ui.Node
    local I18n = env.I18n
    local marks = env.copied[id]
    local copied = marks ~= nil and marks.answer == answer
    local dur = ""
    if duration and duration >= 0 then
        local total = math.round(duration / 1000)
        dur = if total < 60
            then I18n:T("duration.seconds", { count = I18n:Number(total) })
            else I18n:T(
                "duration.minutesSeconds",
                { minutes = I18n:Number(total // 60), seconds = I18n:Number(total % 60) }
            )
    end
    local parts: { string } = {}
    for _, s in { agent, model, dur } do
        if s ~= "" then
            table.insert(parts, s)
        end
    end
    local footer: { ui.Node } = {
        ui.box({ ml = -2 }, {
            copyButton(
                "btw-copy-answer",
                copied,
                I18n:T(if copied then "copied" else "copy"),
                view:Handler(function()
                    env.System:CopyText(answer)
                    local m: { question: string?, answer: string? } = env.copied[id] or {}
                    m.answer = answer
                    env.copied[id] = m
                end)
            ),
        }),
    }
    if #parts > 0 then
        table.insert(footer, meta(table.concat(parts, " \u{b7} "), true))
    end
    return ui.col({ key = "btw-answer", w = "full", mt = 24, hoverGroup = true }, {
        ui.markdown({ source = answer, lineHeight = 25.2, w = "full" }),
        ui.row({ key = "btw-answer-footer", h = 20, mt = 8, gap = 4, align = "center", revealOnHover = true }, footer),
    })
end

-- Why it failed, and Retry (`flex flex-col items-start gap-3 pt-4`).
local function failed(env: Env, view: View, session: string, id: string, failure: machine.Failure): ui.Node
    local I18n = env.I18n
    local detail: string? = nil
    if failure.reason == "empty" then
        detail = I18n:T("error.empty")
    elseif failure.reason == "timeout" then
        detail = I18n:T("error.timeout")
    elseif failure.reason == "error" then
        detail = failure.message
    end
    local words: { ui.Node } =
        { ui.text(I18n:T("error"), { size = 13, weight = "regular", lineHeight = 19.5, color = "text-weak" }) }
    if detail then
        table.insert(
            words,
            ui.text(detail, { size = 12, weight = "regular", lineHeight = 18, color = "text-weak", wrap = true })
        )
    end
    return ui.col({ key = "btw-failed", w = "full", gap = 12, pt = 16 }, {
        ui.col({ gap = 4 }, words),
        ui.row({}, {
            ui.button({
                key = "btw-retry",
                text = I18n:T("retry"),
                variant = "outline",
                size = "small",
                onPress = view:Handler(function()
                    env.retry(session, id)
                end),
            }),
        }),
    })
end

function panel.render(env: Env, view: View, props: PanelProps): ui.Node?
    local session, id = props.sessionId, props.instance or ""
    local phase, info = machine.view(env.state, env.saved(session), session, id)
    local body: { ui.Node } = {}
    if info and phase.kind ~= "missing" then
        local agent = capitalize(info.agent)
        local model = modelLabel(env, info.model)
        local stamp = if info.created then env.I18n:Date(info.created, "time") else ""
        table.insert(body, prompt(env, view, id, info, agent, model, stamp))
        if phase.kind == "running" then
            table.insert(
                body,
                ui.row({ key = "btw-working", role = "status", h = 36, pt = 12, align = "center" }, {
                    ui.shimmer(env.I18n:T("working"), { size = 13, weight = "medium", lineHeight = 16 }),
                })
            )
        elseif phase.kind == "failed" then
            table.insert(body, failed(env, view, session, id, phase.failure))
        elseif phase.kind == "answered" then
            table.insert(body, answered(env, view, id, phase.answer, agent, model, phase.duration))
        end
    end
    return ui.col({ key = "session-btw-panel", w = "full", h = "full", fill = "bg-base" }, {
        ui.col({ key = "btw-scroll", w = "full", grow = 1, minH = 0, scrollY = true, align = "center" }, {
            ui.col({ key = "btw-content", w = "full", maxW = SESSION_W, px = 24, pt = 24, pb = 32 }, body),
        }),
    })
end

return panel
