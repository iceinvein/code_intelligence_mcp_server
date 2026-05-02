//! Helpers for keeping MCP tool responses bounded and self-describing.

use serde::Serialize;
use serde_json::{json, Value};

pub const DEFAULT_MAX_STRING_CHARS: usize = 48_000;

#[derive(Debug, Clone)]
pub struct BudgetedArray<T> {
    pub items: Vec<T>,
    pub total_count: usize,
    pub returned_count: usize,
    pub truncated: bool,
}

impl<T> BudgetedArray<T> {
    pub fn metadata(&self) -> Value {
        json!({
            "total_count": self.total_count,
            "returned_count": self.returned_count,
            "truncated": self.truncated,
        })
    }
}

pub fn clamp_limit(requested: Option<u32>, default: usize, max: usize) -> usize {
    requested
        .map(|value| value as usize)
        .unwrap_or(default)
        .clamp(1, max)
}

pub fn budget_array<T>(mut items: Vec<T>, max_items: usize) -> BudgetedArray<T> {
    let total_count = items.len();
    let max_items = max_items.max(1);
    if items.len() > max_items {
        items.truncate(max_items);
    }
    let returned_count = items.len();
    BudgetedArray {
        items,
        total_count,
        returned_count,
        truncated: total_count > returned_count,
    }
}

pub fn truncate_string(input: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return (String::new(), !input.is_empty());
    }

    let mut chars = input.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    let was_truncated = chars.next().is_some();
    if was_truncated {
        (format!("{truncated}\n... [truncated]"), true)
    } else {
        (truncated, false)
    }
}

pub fn budget_string_field(response: &mut Value, field: &str, max_chars: usize) {
    let Some(text) = response.get(field).and_then(|value| value.as_str()) else {
        return;
    };
    let (truncated_text, truncated) = truncate_string(text, max_chars);
    if !truncated {
        return;
    }
    response[field] = json!(truncated_text);
    response[format!("{field}_budget")] = json!({
        "max_chars": max_chars,
        "truncated": true,
    });
}

pub fn insert_budgeted_array<T: Serialize>(
    response: &mut Value,
    field: &str,
    budgeted: BudgetedArray<T>,
) -> anyhow::Result<()> {
    response[field] = serde_json::to_value(&budgeted.items)?;
    response[format!("{field}_budget")] = budgeted.metadata();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_array_reports_counts() {
        let budgeted = budget_array(vec![1, 2, 3], 2);
        assert_eq!(budgeted.items, vec![1, 2]);
        assert_eq!(budgeted.total_count, 3);
        assert_eq!(budgeted.returned_count, 2);
        assert!(budgeted.truncated);
    }

    #[test]
    fn truncate_string_respects_char_boundaries() {
        let (text, truncated) = truncate_string("aé日b", 3);
        assert_eq!(text, "aé日\n... [truncated]");
        assert!(truncated);
    }
}
