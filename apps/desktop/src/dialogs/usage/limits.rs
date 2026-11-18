//! The usage limits the server reports (`GET /api/experimental/usage/limits`, Desktop's
//! `UsageLimits.Info`), and what the Usage dialog makes of them (`usage/limits.ts`): budgets
//! ordered by what stops the member first, their notes, the callout, the action and the footer.

use serde_json::Value;

/// The usage limits of the OpenCode Console workspace a location is signed in to.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub workspace: Workspace,
    pub go: Option<Go>,
    pub budgets: Vec<Budget>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Workspace {
    pub name: Option<String>,
    /// The member's usage page in OpenCode Console.
    pub usage_url: String,
}

/// One window of the OpenCode Go allowance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meter {
    pub percent: f64,
    /// Go refuses requests until the window resets.
    pub limited: bool,
    pub resets_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Go {
    pub rolling: Meter,
    pub weekly: Meter,
    pub monthly: Meter,
}

/// A model a budget covers.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetModel {
    pub provider: String,
    pub model: String,
    pub name: String,
    /// What a per-model budget's member has spent on this model this month.
    pub spent: Option<f64>,
}

/// A monthly spend limit the workspace sets on the member.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub id: String,
    pub limit: f64,
    pub spent: f64,
    /// `scope: "each"`: every member is limited on their own.
    pub each_member: bool,
    /// `per: "model"`: each model is limited on its own.
    pub per_model: bool,
    /// `coverage: "all"`: every model and billable service.
    pub covers_all: bool,
    pub models: Vec<BudgetModel>,
    pub vendors: Vec<String>,
    pub input_price_at_least: Option<f64>,
    pub picked: bool,
    pub shared: bool,
    pub resets_at: i64,
}

impl Info {
    /// `modelUsage` (anomalyco/opencode#64409): the tightest applicable budget wins.
    pub fn model_left(&self, provider: &str, model: &str, served: bool) -> Option<f64> {
        self.budgets.iter().filter_map(|budget| {
            let spent = if budget.covers_all {
                if !served { return None; }
                budget.spent
            } else {
                let entry = budget.models.iter().find(|m| m.provider == provider && m.model == model)?;
                if budget.per_model { entry.spent.unwrap_or(0.0) } else { budget.spent }
            };
            Some((budget.limit - spent).max(0.0))
        }).min_by(f64::total_cmp)
    }
    /// The `data` of the server's answer; `None` for anything else (`null` is signed out).
    pub fn parse(v: &Value) -> Option<Info> {
        let w = v.get("workspace")?;
        let meter = |m: &Value| Some(Meter { percent: m["percent"].as_f64()?, limited: m["limited"].as_bool()?, resets_at: m["resetsAt"].as_f64()? as i64 });
        let go = v.get("go").filter(|g| !g.is_null()).and_then(|g| Some(Go { rolling: meter(&g["rolling"])?, weekly: meter(&g["weekly"])?, monthly: meter(&g["monthly"])? }));
        let strings = |v: &Value| v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect();
        let budgets = v["budgets"].as_array()?.iter().filter_map(|b| {
            Some(Budget {
                id: b["id"].as_str()?.to_owned(),
                limit: b["limit"].as_f64()?,
                spent: b["spent"].as_f64()?,
                each_member: b["scope"] == "each",
                per_model: b["per"] == "model",
                covers_all: b["coverage"] == "all",
                models: b["models"].as_array().into_iter().flatten().filter_map(|m| Some(BudgetModel {
                    provider: m["providerID"].as_str()?.to_owned(),
                    model: m["modelID"].as_str()?.to_owned(),
                    name: m["name"].as_str()?.to_owned(),
                    spent: m["spent"].as_f64(),
                })).collect(),
                vendors: strings(&b["vendors"]),
                input_price_at_least: b["inputPriceAtLeast"].as_f64(),
                picked: b["picked"].as_bool().unwrap_or(false),
                shared: b["shared"].as_bool().unwrap_or(false),
                resets_at: b["resetsAt"].as_f64()? as i64,
            })
        }).collect();
        Some(Info {
            workspace: Workspace { name: w["name"].as_str().map(str::to_owned), usage_url: w["usageURL"].as_str()?.to_owned() },
            go,
            budgets,
        })
    }
}

/// The model in use, so a per-model budget shows this model's own counter.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentModel {
    pub provider: String,
    pub model: String,
    pub name: String,
}

/// What a budget counts, before it's put into words.
#[derive(Clone, Debug, PartialEq)]
pub enum Label {
    All,
    Vendors { vendors: Vec<String>, each: bool, price: Option<f64> },
    Price { price: f64, each: bool },
    Models { names: Vec<String>, each: bool },
    Each,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    RunsOutFirst,
    Paused,
    IncludesWebSearch,
    PausedBy { shared: bool },
    CurrentModel(String),
}

/// Who spends against a budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sharing {
    You,
    Everyone,
    Group,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: Label,
    pub limit: f64,
    pub per_model: bool,
    pub sharing: Sharing,
    pub notes: Vec<Note>,
    pub reached: bool,
    pub spent: f64,
    pub left: f64,
    /// How much of the limit is spent, 0 to 1.
    pub fill: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Callout {
    Credits,
    Workspace { resets_at: i64 },
    Personal { resets_at: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    OtherModels { vendors: Vec<String> },
    /// `suggestion`: the covered model with the most room left, and its room.
    OtherModelsEach { vendors: Vec<String>, limit: f64, suggestion: Option<(String, f64)> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Footer {
    Resets(i64),
    BudgetsReset(i64),
    CreditsOnly,
}

/// One Go window's row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoRow {
    /// `usage.go.rolling`, `weekly` or `monthly`.
    pub window: &'static str,
    pub meter: Meter,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub workspace: Option<String>,
    pub callout: Option<Callout>,
    pub go: Option<Vec<GoRow>>,
    pub budgets: Vec<Row>,
    /// A callout explains why paid models are paused, so the budgets show as context.
    pub dimmed: bool,
    pub action: Option<Action>,
    pub footer: Footer,
}

/// `usageView`: what the dialog shows for `info`; `credits` marks a workspace out of credits.
pub fn view(info: &Info, model: Option<&CurrentModel>, credits: bool) -> View {
    let mut budgets: Vec<(&Budget, Spend)> = info.budgets.iter().map(|b| (b, spend(b, model))).collect();
    budgets.sort_by(|a, b| a.1.left.total_cmp(&b.1.left));
    let blocking = budgets.iter().find(|(b, s)| s.reached && b.covers_all).map(|(b, _)| *b);
    let callout = if credits {
        Some(Callout::Credits)
    } else {
        blocking.map(|b| if b.shared { Callout::Workspace { resets_at: b.resets_at } } else { Callout::Personal { resets_at: b.resets_at } })
    };
    let any_reached = budgets.iter().any(|(_, s)| s.reached);
    let reached_models = budgets.iter().find(|(b, s)| s.reached && !b.covers_all).map(|(b, _)| *b);
    let resets_at = info.budgets.iter().map(|b| b.resets_at).min().unwrap_or(0);
    let count = budgets.len();
    let rows = budgets
        .iter()
        .enumerate()
        .map(|(index, (b, s))| {
            let mut notes = Vec::new();
            if index == 0 && callout.is_none() && !any_reached && count > 1 {
                notes.push(Note::RunsOutFirst);
            }
            if s.reached && b.covers_all {
                notes.push(Note::IncludesWebSearch);
            }
            // A budget per model names the model it paused instead.
            if s.reached && !b.covers_all && !b.per_model {
                notes.push(Note::Paused);
            }
            if let (false, Some(block)) = (s.reached, blocking) {
                notes.push(Note::PausedBy { shared: block.shared });
            }
            if let Some(name) = &s.current {
                notes.push(Note::CurrentModel(name.clone()));
            }
            Row {
                label: label(b),
                limit: b.limit,
                per_model: b.per_model,
                sharing: if b.each_member { Sharing::You } else if b.shared { Sharing::Everyone } else { Sharing::Group },
                notes,
                reached: s.reached,
                spent: s.spent,
                left: s.left,
                fill: if b.limit > 0.0 { (s.spent / b.limit).min(1.0) } else { 1.0 },
            }
        })
        .collect();
    View {
        workspace: info.workspace.name.clone(),
        callout,
        go: info.go.map(|g| vec![GoRow { window: "rolling", meter: g.rolling }, GoRow { window: "weekly", meter: g.weekly }, GoRow { window: "monthly", meter: g.monthly }]),
        budgets: rows,
        dimmed: callout.is_some(),
        action: if callout.is_some() { None } else { reached_models.map(action) },
        footer: match () {
            _ if info.budgets.is_empty() => Footer::CreditsOnly,
            _ if credits => Footer::BudgetsReset(resets_at),
            _ => Footer::Resets(resets_at),
        },
    }
}

struct Spend {
    current: Option<String>,
    spent: f64,
    reached: bool,
    left: f64,
}

/// The spend a budget's cap compares against: a per-model budget's counter for the model in
/// use, when it covers it.
fn spend(b: &Budget, model: Option<&CurrentModel>) -> Spend {
    let current = model.filter(|_| b.per_model).and_then(|m| b.models.iter().find(|e| e.provider == m.provider && e.model == m.model).map(|e| (m, e)));
    let spent = current.and_then(|(_, e)| e.spent).unwrap_or(b.spent);
    Spend { current: current.map(|(m, _)| m.name.clone()), spent, reached: spent >= b.limit, left: (b.limit - spent).max(0.0) }
}

fn label(b: &Budget) -> Label {
    let each = b.per_model;
    if b.covers_all {
        return Label::All;
    }
    if !b.vendors.is_empty() {
        return Label::Vendors { vendors: b.vendors.clone(), each, price: b.input_price_at_least };
    }
    if let Some(price) = b.input_price_at_least {
        return Label::Price { price, each };
    }
    if b.picked {
        return Label::Models { names: b.models.iter().map(|m| m.name.clone()).collect(), each };
    }
    Label::Each
}

// A per-model budget counts as reached by the model in use, so the model in use never has room
// to suggest.
fn action(b: &Budget) -> Action {
    if !b.per_model {
        return Action::OtherModels { vendors: b.vendors.clone() };
    }
    let mut room: Vec<(String, f64)> = b.models.iter().map(|m| (m.name.clone(), (b.limit - m.spent.unwrap_or(0.0)).max(0.0))).filter(|(_, left)| *left > 0.0).collect();
    room.sort_by(|a, b| b.1.total_cmp(&a.1));
    Action::OtherModelsEach { vendors: b.vendors.clone(), limit: b.limit, suggestion: room.into_iter().next() }
}
