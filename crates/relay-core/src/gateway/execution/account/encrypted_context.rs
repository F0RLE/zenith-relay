//! Pre-output repair for rejected encrypted Responses history.
//!
//! Ordinary execution calls this only for Responses account routes; account-only
//! execution calls it for compact and wake endpoints. It never guesses who owns
//! the ciphertext or attempts to decrypt it.

use crate::gateway::request::responses_item_has_ciphertext;
use serde_json::{Map, Value};
use std::collections::HashSet;

#[derive(serde::Deserialize)]
struct EncryptedRejection {
    code: Option<String>,
    message: Option<String>,
    param: Option<String>,
    error: Option<Box<EncryptedRejection>>,
    response: Option<Box<EncryptedRejection>>,
}

/// Remove rejected encrypted reasoning after a ChatGPT account rejects
/// Responses history.
///
/// The ciphertext's owner cannot be determined locally. Keep it on the first
/// attempt, then use this cleanup only after an explicit upstream rejection.
/// Visible summaries survive without their ciphertext-bound item IDs; items
/// with no visible summary are removed. Ordinary messages and tool history stay.
/// A compaction checkpoint can be the only copy of earlier context: when one is
/// encrypted, refuse the repair without changing any of the request history.
/// Multiple items require an unambiguous provider locator, never a guessed owner.
pub(in crate::gateway::execution) fn drop_rejected_encrypted_context(
    request: &mut Value,
    rejection: &[u8],
) -> bool {
    if rejection.len() > 65_536 {
        return false;
    }
    // Typed fields reject duplicate/conflicting JSON keys before any history edit.
    let Ok(payload) = serde_json::from_slice::<EncryptedRejection>(rejection) else {
        return false;
    };
    let errors: Vec<_> = [
        Some(&payload),
        payload.error.as_deref(),
        payload
            .response
            .as_deref()
            .and_then(|response| response.error.as_deref()),
    ]
    .into_iter()
    .flatten()
    .filter(|error| error.code.is_some() || error.message.is_some())
    .collect();
    if errors.len() != 1 {
        return false;
    }
    let error = errors[0];
    if error.code.as_deref() != Some("invalid_encrypted_content")
        && !(error.code.is_none() && legacy_rejected_item_id(error).is_some())
    {
        return false;
    }
    let Some(input_items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    if input_items.iter().any(is_encrypted_compaction) {
        return false;
    }
    let Some(index) = rejected_reasoning_index(input_items, error) else {
        return false;
    };
    if let Some(visible) = input_items[index]
        .as_object()
        .and_then(visible_item_without_ciphertext)
    {
        input_items[index] = visible;
    } else {
        input_items.remove(index);
    }
    true
}

fn legacy_rejected_item_id(error: &EncryptedRejection) -> Option<&str> {
    let id = error
        .message
        .as_deref()?
        .strip_prefix("Encrypted content for item ")?
        .strip_suffix(
            " could not be verified. Reason: Encrypted content could not be decrypted or parsed.",
        )?;
    (id.starts_with("rs_")
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(id)
}

fn rejected_reasoning_index(input: &[Value], error: &EncryptedRejection) -> Option<usize> {
    let mut ids = HashSet::new();
    for item in input {
        if let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            if !ids.insert(id) {
                return None;
            }
        }
    }
    let candidates: Vec<_> = input
        .iter()
        .enumerate()
        .filter(|(_, item)| is_encrypted_history_item(item))
        .map(|(index, _)| index)
        .collect();
    let mut selected = None;
    if let Some(param) = error.param.as_deref() {
        let index = param
            .strip_prefix("input[")?
            .strip_suffix("].encrypted_content")?;
        let parsed = index.parse::<usize>().ok()?;
        if parsed.to_string() != index || !candidates.contains(&parsed) {
            return None;
        }
        selected = Some(parsed);
    }
    let message = error.message.as_deref().unwrap_or_default();
    if message.starts_with("Encrypted content for item ") {
        let id = legacy_rejected_item_id(error)?;
        let index = candidates
            .iter()
            .copied()
            .find(|&index| input[index].get("id").and_then(Value::as_str) == Some(id))?;
        if selected.is_some_and(|selected| selected != index) {
            return None;
        }
        selected = Some(index);
    }
    if message.starts_with("The encrypted content ")
        && !matches!(
            message,
            "The encrypted content could not be verified"
                | "The encrypted content could not be verified."
                | "The encrypted content could not be verified. Reason: Encrypted content could not be decrypted or parsed."
        )
    {
        let (prefix, suffix) = message
            .strip_prefix("The encrypted content ")?
            .strip_suffix(" could not be verified. Reason: Encrypted content could not be decrypted or parsed.")?
            .split_once("...")?;
        if ![prefix, suffix].into_iter().all(|part| {
            part.len() == 4
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }) {
            return None;
        }
        let matches: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|&index| {
                input[index]
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|text| {
                        text.len() >= 8 && text.starts_with(prefix) && text.ends_with(suffix)
                    })
            })
            .collect();
        if matches.len() != 1 || selected.is_some_and(|index| index != matches[0]) {
            return None;
        }
        selected = Some(matches[0]);
    }
    let selected = selected.or_else(|| (candidates.len() == 1).then(|| candidates[0]))?;
    if let Some(ciphertext) = input[selected].get("encrypted_content") {
        if candidates
            .iter()
            .filter(|&&index| input[index].get("encrypted_content") == Some(ciphertext))
            .count()
            != 1
        {
            return None;
        }
    }
    Some(selected)
}

fn is_encrypted_compaction(history_item: &Value) -> bool {
    responses_item_has_ciphertext(history_item.get("encrypted_content"))
        && (history_item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| crate::protocol::is_compaction_checkpoint_type(kind.trim()))
            || history_item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("cmp_")))
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
    kind == "reasoning"
        || item_object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|response_item_id| response_item_id.starts_with("rs_"))
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
