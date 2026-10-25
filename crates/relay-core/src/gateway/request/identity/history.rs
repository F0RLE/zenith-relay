//! Bounded, caller-scoped soft prefix hints. No prompts or hashes are persisted
//! here, and a hint never retrieves history or authorizes opaque continuation.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::sync::OnceLock;

const MAX_PREFIX_BYTES: usize = 64 * 1024;
const MIN_USER_TEXT_BYTES: usize = 32;
static SALT: OnceLock<Option<[u8; 32]>> = OnceLock::new();

struct BoundedHash {
    hash: Sha256,
    bytes: usize,
}

impl Write for BoundedHash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PREFIX_BYTES.saturating_sub(self.bytes) {
            return Err(io::Error::other("soft affinity prefix limit"));
        }
        self.hash.update(bytes);
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(in crate::gateway) fn history_affinity_fingerprint(
    local_key: &str,
    request: &Value,
) -> Option<String> {
    use ring::rand::SecureRandom;
    let salt = SALT
        .get_or_init(|| {
            let mut salt = [0; 32];
            ring::rand::SystemRandom::new().fill(&mut salt).ok()?;
            Some(salt)
        })
        .as_ref()?;
    let mut instructions = Vec::new();
    for name in [
        "instructions",
        "system",
        "systemInstruction",
        "system_instruction",
    ] {
        if let Some(value) = request.get(name) {
            instructions.push(value);
        }
    }
    let user = if let Some(Value::String(text)) = request.get("input") {
        if text.trim().is_empty() {
            return None;
        }
        request.get("input")?
    } else {
        let messages = request
            .get("messages")
            .or_else(|| request.get("contents"))
            .or_else(|| request.get("input"))?
            .as_array()?;
        let mut user = None;
        for message in messages.iter().take(64) {
            match message.get("role").and_then(Value::as_str) {
                Some("system" | "developer") if user.is_none() => instructions.push(message),
                Some("user") => {
                    user = Some(message);
                    break;
                }
                // A shortened history without its opening user turn must not
                // alias an unrelated context merely on an assistant/tool tail.
                Some("assistant" | "model" | "tool") => return None,
                _ => return None,
            }
        }
        user?
    };
    let text = user
        .get("content")
        .or_else(|| user.get("parts"))
        .unwrap_or(user);
    let text_bytes = text.as_str().map_or_else(
        || {
            text.as_array().map_or(0, |parts| {
                parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .map(|text| text.trim().len())
                    .sum()
            })
        },
        |text| text.trim().len(),
    );
    if text_bytes < MIN_USER_TEXT_BYTES {
        return None;
    }
    let mut writer = BoundedHash {
        hash: Sha256::new(),
        bytes: 0,
    };
    writer.hash.update(salt);
    serde_json::to_writer(
        &mut writer,
        &("soft-prefix-v1", local_key, instructions, user),
    )
    .ok()?;
    Some(format!("prefix_{}", hex::encode(writer.hash.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn anonymous_forks_keep_a_soft_prefix_without_sharing_keys_or_instructions() {
        let base = json!({"instructions":"synthetic rules","input":[{"role":"user","content":"Synthetic opening task with enough context to identify a conversation branch."}]});
        let mut branch = base.clone();
        branch["input"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":"first branch"}),
            json!({"role":"user","content":"continue"}),
        ]);
        let key = history_affinity_fingerprint("key-a", &base).unwrap();
        assert_eq!(
            history_affinity_fingerprint("key-a", &branch),
            Some(key.clone())
        );
        assert_ne!(
            history_affinity_fingerprint("key-b", &base),
            Some(key.clone())
        );
        branch["instructions"] = "different rules".into();
        assert_ne!(history_affinity_fingerprint("key-a", &branch), Some(key));
    }

    #[test]
    fn oversized_prefixes_or_ambiguous_history_do_not_create_bindings() {
        assert!(history_affinity_fingerprint("k", &json!({"input":"Hi"})).is_none());
        assert!(
            history_affinity_fingerprint("k", &json!({"input":"x".repeat(MAX_PREFIX_BYTES)}))
                .is_none()
        );
        assert!(history_affinity_fingerprint("k", &json!({"messages":[{"role":"assistant","content":"orphan"},{"role":"user","content":"tail"}]})).is_none());
        assert!(history_affinity_fingerprint("k", &json!({"input":[]})).is_none());
    }
}
