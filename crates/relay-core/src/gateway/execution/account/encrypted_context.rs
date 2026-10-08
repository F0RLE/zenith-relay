//! Pre-output repair for rejected encrypted Responses history.
//!
//! Ordinary execution calls this only for Responses account routes; account-only
//! execution calls it for compact and wake endpoints. It never guesses who owns
//! the ciphertext or attempts to decrypt it.

use crate::gateway::request::responses_item_has_ciphertext;
use serde_json::{Map, Value};

/// Remove encrypted reasoning and compaction items after a ChatGPT account
/// rejects Responses history.
///
/// The ciphertext's owner cannot be determined locally. Keep it on the first
/// attempt, then use this cleanup only after an explicit upstream rejection.
/// Visible summaries survive without their ciphertext-bound item IDs; items
/// with no visible summary are removed. Ordinary messages and tool history stay.
pub(in crate::gateway::execution) fn drop_rejected_encrypted_context(request: &mut Value) -> bool {
    let Some(input_items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut changed = false;
    input_items.retain_mut(|history_item| {
        if !is_encrypted_history_item(history_item) {
            return true;
        }
        changed = true;
        if let Some(visible) = history_item
            .as_object()
            .and_then(visible_item_without_ciphertext)
        {
            *history_item = visible;
            true
        } else {
            false
        }
    });
    changed
}

fn is_encrypted_history_item(history_item: &Value) -> bool {
    let Some(item_object) = history_item.as_object() else {
        return false;
    };
    if !responses_item_has_ciphertext(item_object.get("encrypted_content")) {
        return false;
    }
    let kind = item_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("");
    matches!(kind, "reasoning" | "compaction" | "compaction_summary")
        || item_object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|response_item_id| {
                response_item_id.starts_with("rs_") || response_item_id.starts_with("cmp_")
            })
}

fn visible_item_without_ciphertext(item_object: &Map<String, Value>) -> Option<Value> {
    let mut visible = item_object.clone();
    visible.remove("encrypted_content");
    visible.remove("id");
    has_visible_text(&visible).then_some(Value::Object(visible))
}

fn has_visible_text(item_object: &Map<String, Value>) -> bool {
    ["summary", "content"].into_iter().any(|field| {
        item_object
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
                })
            })
    })
}

#[cfg(test)]
mod tests;
