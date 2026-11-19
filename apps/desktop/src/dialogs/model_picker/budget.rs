//! Model-picker inline budget warnings (`modelUsage`, anomalyco/opencode#64409).
//!
//! `session_view::pickers` (`PickerHost::model_budget`) queries the tightest applicable Console
//! budget synchronously for each model row while rendering the picker. This stays in Rust beside
//! the model picker rather than adding a usage-specific host hook to the plugin contract.

use std::collections::HashMap;

use opencode_kernel::Slot;
use rune::Tx;
use serde_json::Value;

use crate::App;

pub(crate) static PICKER_BUDGETS: Slot<PickerBudgets> = Slot::new();

#[derive(Default)]
pub(crate) struct PickerBudgets {
    cache: HashMap<String, Vec<BudgetLimit>>,
}

struct BudgetModel {
    provider: String,
    model: String,
    spent: Option<f64>,
}

struct BudgetLimit {
    limit: f64,
    spent: f64,
    per_model: bool,
    covers_all: bool,
    models: Vec<BudgetModel>,
}

fn parse_budgets(v: &Value) -> Option<Vec<BudgetLimit>> {
    let v = v.get("data").unwrap_or(v);
    let w = v.get("workspace")?;
    w["usageURL"].as_str()?;
    let budgets = v["budgets"]
        .as_array()?
        .iter()
        .filter_map(|b| {
            Some(BudgetLimit {
                limit: b["limit"].as_f64()?,
                spent: b["spent"].as_f64()?,
                per_model: b["per"] == "model",
                covers_all: b["coverage"] == "all",
                models: b["models"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        Some(BudgetModel {
                            provider: m["providerID"].as_str()?.to_owned(),
                            model: m["modelID"].as_str()?.to_owned(),
                            spent: m["spent"].as_f64(),
                        })
                    })
                    .collect(),
            })
        })
        .collect();
    Some(budgets)
}

fn model_left(budgets: &[BudgetLimit], provider: &str, model: &str, served: bool) -> Option<f64> {
    budgets
        .iter()
        .filter_map(|budget| {
            let spent = if budget.covers_all {
                if !served {
                    return None;
                }
                budget.spent
            } else {
                let entry = budget.models.iter().find(|m| m.provider == provider && m.model == model)?;
                if budget.per_model { entry.spent.unwrap_or(0.0) } else { budget.spent }
            };
            Some((budget.limit - spent).max(0.0))
        })
        .min_by(f64::total_cmp)
}

fn limits_endpoint(directory: &str) -> String {
    let encoded: String = directory
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("/api/experimental/usage/limits?location%5Bdirectory%5D={encoded}")
}

impl App {
    pub(crate) fn refresh_picker_usage(&mut self) {
        let directory = self.composer_directory();
        if !self.live() {
            let parsed = self.fixture_run.fixture.get("usage_limits").and_then(parse_budgets);
            if let Some(budgets) = parsed {
                self.kernel.slots.get_mut(&PICKER_BUDGETS).cache.insert(directory, budgets);
            }
            return;
        }
        self.job(move |srv| {
            let path = limits_endpoint(&directory);
            let answer = srv.get(&path).map_err(|e| e.to_string());
            Box::new(move |s: &mut App, tx: &mut Tx| {
                if let Ok(value) = answer {
                    let cache = &mut s.kernel.slots.get_mut(&PICKER_BUDGETS).cache;
                    if let Some(budgets) = parse_budgets(&value) {
                        cache.insert(directory, budgets);
                    } else {
                        cache.remove(&directory);
                    }
                    tx.instant();
                }
            })
        });
    }

    pub(crate) fn picker_budget_label(&self, key: &str) -> Option<(String, bool)> {
        let model = self.composer_ext.pickers.models.iter().find(|m| m.key == key && !m.free)?;
        let served = model.provider != "opencode-go"
            && (model.provider == "opencode"
                || self
                    .provider_infos(None)
                    .unwrap_or_default()
                    .iter()
                    .any(|p| p.id == model.provider && p.integration.as_deref() == Some("opencode")));
        let budgets = self.kernel.slots.get(&PICKER_BUDGETS).cache.get(&self.composer_directory())?;
        let left = model_left(budgets, &model.provider, &model.id, served)?;
        let label = if left <= 0.0 {
            opencode_i18n::t!("usage.budget.reached").into()
        } else {
            opencode_i18n::t!("usage.budget.left", amount = opencode_i18n::usd(left, 2))
        };
        Some((label, left <= 0.0))
    }
}
