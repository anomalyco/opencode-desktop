//! The latest turn's numbers, as the TUI's last assistant footer shows them (`bar.tsx`,
//! `util/session-turn.ts`, `util/duration.ts`): how long the turn took, and its tokens per second.

use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    /// Compact elapsed time; `None` while the step runs.
    pub duration: Option<String>,
    pub tps: Option<f64>,
}

/// The numbers of the latest assistant message that ended its turn, failed or is retrying;
/// `None` when there is none. `messages` are in the session's order.
pub fn latest(messages: &[Value]) -> Option<Turn> {
    let index = messages.iter().rposition(|m| {
        let finish = m["finish"].as_str();
        m["type"] == "assistant" && (truthy(&m["retry"]) || truthy(&m["error"]) || finish.is_some_and(|f| !f.is_empty() && f != "tool-calls" && f != "unknown"))
    })?;
    let legacy = !messages.iter().any(|m| m["type"] == "idle");
    // The TUI hides a zero duration, which means the step has not completed.
    let elapsed = duration(messages, index, legacy);
    Some(Turn { duration: (elapsed > 0).then(|| compact(elapsed)), tps: tokens_per_second(messages, index, legacy) })
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Bool(false)) && v.as_str() != Some("")
}

fn time(m: &Value, field: &str) -> Option<i64> {
    m["time"][field].as_f64().map(|t| t as i64)
}

/// Milliseconds from the prompt that started the turn until message `index` completed, or 0
/// while it runs.
fn duration(messages: &[Value], index: usize, legacy: bool) -> i64 {
    let message = &messages[index];
    let Some(completed) = time(message, "completed") else { return 0 };
    let input = input_index(messages, index, legacy).and_then(|i| time(&messages[i], "created"));
    (completed - input.or_else(|| time(message, "created")).unwrap_or(completed)).max(0)
}

/// Output and reasoning tokens per second across the turn's steps up to message `index`, or
/// `None` until every step has streamed.
fn tokens_per_second(messages: &[Value], index: usize, legacy: bool) -> Option<f64> {
    let end = index + 1;
    let start = input_index(messages, end, legacy).map_or(0, |i| i + 1);
    let steps: Vec<&Value> = messages[start..end].iter().filter(|m| m["type"] == "assistant").collect();
    let durations: Vec<i64> = steps.iter().filter_map(|s| Some((time(s, "streamed")? - time(s, "created")?).max(0))).collect();
    if steps.is_empty() || durations.len() != steps.len() {
        return None;
    }
    let output: f64 = steps.iter().map(|s| s["tokens"]["output"].as_f64().unwrap_or(0.0) + s["tokens"]["reasoning"].as_f64().unwrap_or(0.0)).sum();
    let total: i64 = durations.iter().sum();
    if output <= 0.0 || total <= 0 {
        return None;
    }
    // Aggregate before dividing so each step is weighted by its provider-active duration.
    Some(output / (total as f64 / 1000.0))
}

/// The turn's prompt before `end`: the nearest input in legacy sessions, otherwise the first
/// input after the previous idle marker.
fn input_index(messages: &[Value], end: usize, legacy: bool) -> Option<usize> {
    let mut input = None;
    for index in (0..end).rev() {
        match messages[index]["type"].as_str() {
            Some("idle") => return input,
            Some("user" | "synthetic") if legacy => return Some(index),
            Some("user" | "synthetic") => input = Some(index),
            _ => {}
        }
    }
    input
}

/// Compact elapsed time: `850ms`, `12.4s`, `3m 5s`, `2h 10m` or `1d 4h`.
pub fn compact(ms: i64) -> String {
    match ms {
        ..1_000 => format!("{ms}ms"),
        ..60_000 => format!("{:.1}s", ms as f64 / 1000.0),
        ..3_600_000 => format!("{}m {}s", ms / 60_000, ms % 60_000 / 1000),
        ..86_400_000 => format!("{}h {}m", ms / 3_600_000, ms % 3_600_000 / 60_000),
        _ => format!("{}d {}h", ms / 86_400_000, ms % 86_400_000 / 3_600_000),
    }
}
