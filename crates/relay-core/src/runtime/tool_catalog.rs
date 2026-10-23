//! Volatile preload hints, not provider continuation or tool authorization.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const MAX_ENTRIES: usize = 256;
const MAX_NAMES: usize = 64;
const MAX_NAME_BYTES: usize = 256;
const MAX_PREFIX_BYTES: usize = 32 * 1024;
const TTL_MS: u64 = 60 * 60 * 1000;

#[derive(Default)]
pub(crate) struct ToolCatalogStore {
    entries: Mutex<BTreeMap<[u8; 32], Entry>>,
}

#[derive(Clone)]
struct Entry {
    names: Arc<BTreeSet<String>>,
    prefix: Option<Arc<([u8; 32], String)>>,
    expires_at: u64,
}

fn key(local_key_id: &str, model: &str, call_id: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    for part in [local_key_id, model, call_id] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    digest.finalize().into()
}

impl ToolCatalogStore {
    pub(crate) fn prefix_scope(
        &self,
        request: &Value,
        client_context_id: Option<&str>,
    ) -> Option<[u8; 32]> {
        let context = client_context_id.filter(|context| !context.is_empty())?;
        let mut digest = Sha256::new();
        digest.update((context.len() as u64).to_be_bytes());
        digest.update(context.as_bytes());
        for value in std::iter::once(&request["instructions"]).chain(
            request["input"]
                .as_array()
                .into_iter()
                .flatten()
                .take_while(|item| matches!(item["role"].as_str(), Some("system" | "developer"))),
        ) {
            let bytes = serde_json::to_vec(value).ok()?;
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        Some(digest.finalize().into())
    }

    pub(crate) fn load_prefix(
        &self,
        local_key_id: &str,
        model: &str,
        native_input: &Value,
        scope: &[u8; 32],
        now_ms: u64,
    ) -> Option<String> {
        let entry = self.load_entry(local_key_id, model, native_input, now_ms)?;
        let prefix = entry.prefix?;
        (prefix.0 == *scope).then(|| prefix.1.clone())
    }

    pub(crate) fn load(
        &self,
        local_key_id: &str,
        model: &str,
        native_input: &Value,
        now_ms: u64,
    ) -> BTreeSet<String> {
        self.load_entry(local_key_id, model, native_input, now_ms)
            .map(|entry| (*entry.names).clone())
            .unwrap_or_default()
    }

    pub(crate) fn checkpoint_for_history(
        &self,
        local_key_id: &str,
        model: &str,
        native_input: &Value,
    ) -> Option<[u8; 32]> {
        native_input.as_array()?.iter().rev().find_map(|item| {
            if !matches!(
                item["type"].as_str(),
                Some(
                    "function_call"
                        | "custom_tool_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                )
            ) {
                return None;
            }
            item["call_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(|call_id| key(local_key_id, model, call_id))
        })
    }

    pub(crate) fn remember_checkpoint(
        &self,
        checkpoint: [u8; 32],
        names: &BTreeSet<String>,
        prefix: Option<(&[u8; 32], &str)>,
        now_ms: u64,
    ) {
        self.remember_keys(&[checkpoint], names, prefix, now_ms);
    }

    fn load_entry(
        &self,
        local_key_id: &str,
        model: &str,
        native_input: &Value,
        now_ms: u64,
    ) -> Option<Entry> {
        // Prepared history has already validated and collapsed batch child IDs.
        let items = native_input.as_array()?;
        let mut entries = crate::poison::mutex(&self.entries);
        entries.retain(|_, entry| entry.expires_at > now_ms);
        for item in items.iter().rev() {
            if !matches!(
                item["type"].as_str(),
                Some(
                    "function_call"
                        | "custom_tool_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                )
            ) {
                continue;
            }
            if let Some(call_id) = item["call_id"].as_str() {
                if let Some(entry) = entries.get(&key(local_key_id, model, call_id)) {
                    return Some(entry.clone());
                }
            }
        }
        None
    }

    pub(crate) fn remember(
        &self,
        local_key_id: &str,
        model: &str,
        response: &Value,
        names: &BTreeSet<String>,
        prefix: Option<(&[u8; 32], &str)>,
        now_ms: u64,
    ) {
        if response["status"] != "completed" {
            return;
        }
        let Some(items) = response["output"].as_array() else {
            return;
        };
        let checkpoints: Vec<_> = items
            .iter()
            .filter_map(|item| {
                if !matches!(
                    item["type"].as_str(),
                    Some("function_call" | "custom_tool_call")
                ) {
                    return None;
                }
                item["call_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(|call_id| key(local_key_id, model, call_id))
            })
            .collect();
        self.remember_keys(&checkpoints, names, prefix, now_ms);
    }

    fn remember_keys(
        &self,
        checkpoints: &[[u8; 32]],
        names: &BTreeSet<String>,
        prefix: Option<(&[u8; 32], &str)>,
        now_ms: u64,
    ) {
        if checkpoints.is_empty() {
            return;
        }
        let names: BTreeSet<_> = names
            .iter()
            .filter(|name| name.len() <= MAX_NAME_BYTES)
            .take(MAX_NAMES)
            .cloned()
            .collect();
        if names.is_empty() {
            return;
        }
        let names = Arc::new(names);
        let prefix = prefix
            .filter(|(_, text)| !text.is_empty() && text.len() <= MAX_PREFIX_BYTES)
            .map(|(scope, text)| Arc::new((*scope, text.to_string())));
        let mut entries = crate::poison::mutex(&self.entries);
        entries.retain(|_, entry| entry.expires_at > now_ms);
        for &key in checkpoints {
            if entries.len() >= MAX_ENTRIES && !entries.contains_key(&key) {
                if let Some(oldest) = entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires_at)
                    .map(|(key, _)| *key)
                {
                    entries.remove(&oldest);
                }
            }
            entries.insert(
                key,
                Entry {
                    names: names.clone(),
                    prefix: prefix.clone(),
                    expires_at: now_ms.saturating_add(TTL_MS),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn completed_lookup_hints_are_scoped_to_existing_history_without_a_final_answer() {
        let store = ToolCatalogStore::default();
        let input = json!([
            {"type":"function_call","call_id":"call_prior"},
            {"type":"function_call_output","call_id":"call_prior","output":"done"}
        ]);
        let checkpoint = store
            .checkpoint_for_history("client-a", "model-a", &input)
            .unwrap();
        let names = BTreeSet::from(["files.read".to_string()]);
        store.remember_checkpoint(checkpoint, &names, None, 1);
        assert_eq!(store.load("client-a", "model-a", &input, 2), names);
        assert!(store.load("client-b", "model-a", &input, 2).is_empty());
        assert!(store.load("client-a", "model-b", &input, 2).is_empty());
        let foreign = json!([
            {"type":"function_call_output","call_id":"call_other","output":"done"}
        ]);
        assert!(store.load("client-a", "model-a", &foreign, 2).is_empty());
        assert!(store
            .load("client-a", "model-a", &input, TTL_MS + 1)
            .is_empty());
        let no_history = json!([
            {"role":"user","content":"No tool checkpoint"}
        ]);
        assert!(store
            .checkpoint_for_history("client-a", "model-a", &no_history)
            .is_none());
    }

    #[test]
    fn historical_prefix_requires_matching_scope_history_key_and_model() {
        let store = ToolCatalogStore::default();
        let response = json!({"status":"completed","output":[{"type":"function_call","call_id":"call_synthetic"}]});
        let request = json!({"model":"synthetic","instructions":"Keep synthetic constraints","input":[
            {"role":"developer","content":"Synthetic project rules"},
            {"type":"function_call_output","call_id":"call_synthetic","output":"ok"}
        ]});
        let names = BTreeSet::from(["files.read".to_string()]);
        let scope = store.prefix_scope(&request, Some("thread-a")).unwrap();
        store.remember(
            "client-a",
            "synthetic",
            &response,
            &names,
            Some((&scope, "synthetic prefix")),
            1,
        );
        assert_eq!(
            store
                .load_prefix("client-a", "synthetic", &request["input"], &scope, 2)
                .as_deref(),
            Some("synthetic prefix")
        );
        assert!(store
            .load_prefix("client-b", "synthetic", &request["input"], &scope, 2)
            .is_none());
        let other_scope = store.prefix_scope(&request, Some("thread-b")).unwrap();
        assert!(store
            .load_prefix("client-a", "synthetic", &request["input"], &other_scope, 2)
            .is_none());
        assert!(store.prefix_scope(&request, None).is_none());
        for (field, value) in [("model", json!("other")), ("input", json!([]))] {
            let mut changed = request.clone();
            changed[field] = value;
            assert!(store
                .load_prefix(
                    "client-a",
                    changed["model"].as_str().unwrap(),
                    &changed["input"],
                    &scope,
                    2,
                )
                .is_none());
        }
        let mut changed = request.clone();
        changed["instructions"] = json!("Different instructions");
        assert_ne!(store.prefix_scope(&changed, Some("thread-a")), Some(scope));
        changed = request.clone();
        changed["input"][0]["content"] = json!("Different project rules");
        assert_ne!(store.prefix_scope(&changed, Some("thread-a")), Some(scope));
        assert!(store
            .load_prefix(
                "client-a",
                "synthetic",
                &request["input"],
                &scope,
                TTL_MS + 1
            )
            .is_none());
    }

    #[test]
    fn oversized_or_incomplete_prefixes_are_not_retained() {
        let store = ToolCatalogStore::default();
        let request = json!({"model":"synthetic","input":[{"type":"function_call_output","call_id":"call_synthetic","output":"ok"}]});
        let scope = store.prefix_scope(&request, Some("thread")).unwrap();
        let names = BTreeSet::from(["files.read".to_string()]);
        let mut response = json!({"status":"incomplete","output":[{"type":"function_call","call_id":"call_synthetic"}]});
        store.remember(
            "client",
            "synthetic",
            &response,
            &names,
            Some((&scope, "prefix")),
            1,
        );
        assert!(store
            .load_prefix("client", "synthetic", &request["input"], &scope, 2)
            .is_none());
        response["status"] = json!("completed");
        store.remember(
            "client",
            "synthetic",
            &response,
            &names,
            Some((&scope, &"x".repeat(MAX_PREFIX_BYTES + 1))),
            2,
        );
        assert!(store
            .load_prefix("client", "synthetic", &request["input"], &scope, 3)
            .is_none());
        assert_eq!(
            store.load("client", "synthetic", &request["input"], 3),
            names
        );
    }

    #[test]
    fn hints_require_the_same_client_model_and_history_and_expire() {
        let store = ToolCatalogStore::default();
        let response = json!({"status":"completed","output":[{"type":"function_call","call_id":"call_synthetic"}]});
        let request = json!({"model":"synthetic","input":[{"type":"function_call_output","call_id":"call_synthetic","output":"ok"}]});
        let names = BTreeSet::from(["files.read".to_string(), "files.write".to_string()]);
        store.remember("client-a", "synthetic", &response, &names, None, 1);
        assert_eq!(
            store.load("client-a", "synthetic", &request["input"], 2),
            names
        );
        assert!(store
            .load("client-b", "synthetic", &request["input"], 2)
            .is_empty());
        let mut other = request.clone();
        other["model"] = json!("other");
        assert!(store
            .load(
                "client-a",
                other["model"].as_str().unwrap(),
                &other["input"],
                2
            )
            .is_empty());
        other["model"] = json!("synthetic");
        other["input"][0]["call_id"] = json!("different_chat");
        assert!(store
            .load(
                "client-a",
                other["model"].as_str().unwrap(),
                &other["input"],
                2
            )
            .is_empty());
        assert!(store
            .load("client-a", "synthetic", &request["input"], TTL_MS + 1)
            .is_empty());
    }

    #[test]
    fn hints_are_bounded_and_incomplete_outputs_do_not_populate_them() {
        let store = ToolCatalogStore::default();
        let mut names: BTreeSet<_> = (0..MAX_NAMES + 1).map(|n| format!("tool_{n:03}")).collect();
        names.insert("x".repeat(MAX_NAME_BYTES + 1));
        for n in 0..MAX_ENTRIES + 1 {
            let response = json!({"status":"completed","output":[{"type":"function_call","call_id":format!("call_{n}")}]});
            store.remember("client", "model", &response, &names, None, n as u64);
        }
        let entries = crate::poison::mutex(&store.entries);
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert!(!entries.contains_key(&key("client", "model", "call_0")));
        assert!(entries.values().all(|entry| entry.names.len() == MAX_NAMES
            && entry.names.iter().all(|name| name.len() <= MAX_NAME_BYTES)));
        drop(entries);
        let response =
            json!({"status":"incomplete","output":[{"type":"function_call","call_id":"partial"}]});
        store.remember("client", "model", &response, &names, None, 1000);
        assert!(
            !crate::poison::mutex(&store.entries).contains_key(&key("client", "model", "partial"))
        );
    }
}
