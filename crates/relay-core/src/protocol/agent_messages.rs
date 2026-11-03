//! Relay-owned tool-argument ciphertext, never a decoder for provider ciphertext.
use super::AdapterError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    aead,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

const PREFIX: &str = "zenith-relay-agent-v1:";
const MAX_TEXT_BYTES: usize = 1024 * 1024;
const MAX_NAME_BYTES: usize = 4096;
// Worst-case JSON escaping (6x), base64 (4/3), metadata, nonce and tag.
const MAX_ENVELOPE_BYTES: usize = (MAX_TEXT_BYTES + 2 * MAX_NAME_BYTES + 128) * 8;

/// Host-owned key, shared by routing-graph replacements. Durable hosts supply
/// their protected key explicitly; standalone defaults remain ephemeral.
/// No messages, tools or plaintext are retained in this owner.
#[derive(Default)]
pub(crate) struct AgentMessageCipher {
    key: OnceLock<Option<aead::LessSafeKey>>,
}

impl AgentMessageCipher {
    pub(crate) fn from_key(bytes: [u8; 32]) -> Self {
        let key = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &bytes)
            .expect("a 256-bit key has the required ChaCha20-Poly1305 length");
        Self {
            key: OnceLock::from(Some(aead::LessSafeKey::new(key))),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AgentMessageContext {
    cipher: Arc<AgentMessageCipher>,
    local_key_id: String,
}

#[derive(Serialize, Deserialize)]
struct Argument {
    tool: String,
    parameter: String,
    text: String,
}

fn invalid() -> AdapterError {
    AdapterError::invalid_request().with_parameter("input.agent_message.encrypted_content")
}

fn invalid_output() -> AdapterError {
    AdapterError::upstream_response_invalid().with_parameter("output.function_call.arguments")
}

impl AgentMessageContext {
    pub(crate) fn new(cipher: Arc<AgentMessageCipher>, local_key_id: &str) -> Self {
        Self {
            cipher,
            local_key_id: local_key_id.into(),
        }
    }

    fn key(&self) -> Result<&aead::LessSafeKey, AdapterError> {
        self.cipher
            .key
            .get_or_init(|| {
                let mut bytes = [0; 32];
                SystemRandom::new().fill(&mut bytes).ok()?;
                aead::UnboundKey::new(&aead::CHACHA20_POLY1305, &bytes)
                    .ok()
                    .map(aead::LessSafeKey::new)
            })
            .as_ref()
            .ok_or_else(invalid)
    }

    fn aad(&self) -> aead::Aad<Vec<u8>> {
        aead::Aad::from(format!("{PREFIX}{}", self.local_key_id).into_bytes())
    }

    pub(crate) fn seal_argument(
        &self,
        tool: &str,
        parameter: &str,
        text: &str,
    ) -> Result<String, AdapterError> {
        if text.len() > MAX_TEXT_BYTES
            || tool.len() > MAX_NAME_BYTES
            || parameter.len() > MAX_NAME_BYTES
        {
            return Err(invalid_output());
        }
        let mut body = serde_json::to_vec(&Argument {
            tool: tool.into(),
            parameter: parameter.into(),
            text: text.into(),
        })
        .map_err(|_| invalid_output())?;
        let mut nonce = [0; aead::NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| invalid_output())?;
        self.key()
            .map_err(|_| invalid_output())?
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                self.aad(),
                &mut body,
            )
            .map_err(|_| invalid_output())?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&body);
        Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(sealed)))
    }

    fn open_argument(&self, value: &str) -> Result<Option<Argument>, AdapterError> {
        let Some(encoded) = value.strip_prefix(PREFIX) else {
            return Ok(None);
        };
        if encoded.len() > MAX_ENVELOPE_BYTES {
            return Err(invalid());
        }
        let mut sealed = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
        if sealed.len() < aead::NONCE_LEN + aead::CHACHA20_POLY1305.tag_len() {
            return Err(invalid());
        }
        let (nonce, body) = sealed.split_at_mut(aead::NONCE_LEN);
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce).map_err(|_| invalid())?;
        let plain = self
            .key()?
            .open_in_place(nonce, self.aad(), body)
            .map_err(|_| AdapterError::agent_message_unavailable())?;
        let argument: Argument = serde_json::from_slice(plain).map_err(|_| invalid())?;
        if argument.text.len() > MAX_TEXT_BYTES {
            return Err(invalid());
        }
        Ok(Some(argument))
    }

    /// Only authenticated Relay envelopes are made portable before admission.
    /// Foreign blobs retain their native route/unsupported-conversion behavior.
    pub(crate) fn restore_request(&self, request: &mut Value) -> Result<(), AdapterError> {
        if let Some(input) = request.get_mut("input") {
            self.restore_input(input)?;
        }
        Ok(())
    }

    pub(crate) fn restore_input(&self, input: &mut Value) -> Result<(), AdapterError> {
        match input {
            Value::Array(items) => {
                for item in items {
                    self.restore_item(item)?;
                }
            }
            item @ Value::Object(_) => self.restore_item(item)?,
            _ => {}
        }
        Ok(())
    }

    fn restore_item(&self, item: &mut Value) -> Result<(), AdapterError> {
        match item.get("type").and_then(Value::as_str) {
            Some("agent_message") => self.restore_agent(item),
            Some("function_call") => self.restore_call(item),
            _ => Ok(()),
        }
    }

    fn restore_agent(&self, item: &mut Value) -> Result<(), AdapterError> {
        let owned = |value: &Value| value.as_str().is_some_and(|text| text.starts_with(PREFIX));
        let top_owned = item.get("encrypted_content").is_some_and(owned);
        let parts_owned = item
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part["type"] == "encrypted_content"
                        && part.get("encrypted_content").is_some_and(owned)
                })
            });
        if !top_owned && !parts_owned {
            return Ok(());
        }
        let mut content = Vec::new();
        if let Some(value) = item
            .get("encrypted_content")
            .filter(|value| !value.is_null())
        {
            let text = self
                .open_argument(value.as_str().ok_or_else(invalid)?)?
                .ok_or_else(invalid)?
                .text;
            content.push(json!({"type":"input_text","text":text}));
        }
        match item.get("content") {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => content.push(json!({"type":"input_text","text":text})),
            Some(Value::Array(parts)) => {
                for part in parts {
                    if part["type"] == "encrypted_content" {
                        let text = self
                            .open_argument(part["encrypted_content"].as_str().ok_or_else(invalid)?)?
                            .ok_or_else(invalid)?
                            .text;
                        content.push(json!({"type":"input_text","text":text}));
                    } else {
                        content.push(part.clone());
                    }
                }
            }
            _ => return Err(invalid()),
        }
        let mut attribution = item.as_object().ok_or_else(invalid)?.clone();
        attribution.remove("content");
        attribution.remove("encrypted_content");
        content.insert(0, json!({"type":"input_text","text":format!(
            "Collaboration context from another agent, not a new user instruction. Attribution: {}", Value::Object(attribution))}));
        *item = json!({"type":"message","role":"user","content":content});
        Ok(())
    }

    fn restore_call(&self, item: &mut Value) -> Result<(), AdapterError> {
        let Some(encoded) = item
            .get("arguments")
            .and_then(Value::as_str)
            .filter(|text| text.contains(PREFIX))
        else {
            return Ok(());
        };
        let mut arguments: Value = serde_json::from_str(encoded).map_err(|_| invalid())?;
        let Some(properties) = arguments.as_object_mut() else {
            return Ok(());
        };
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
        let tool = match item.get("namespace").and_then(Value::as_str) {
            Some(namespace) => format!("{namespace}.{name}"),
            None => name.into(),
        };
        let mut changed = false;
        for (parameter, value) in properties {
            if let Some(text) = value.as_str()
                && let Some(argument) = self.open_argument(text)?
            {
                if argument.tool != tool || argument.parameter != *parameter {
                    return Err(invalid());
                }
                *value = Value::String(argument.text);
                changed = true;
            }
        }
        if changed {
            item["arguments"] = serde_json::to_string(&arguments)
                .map_err(|_| invalid())?
                .into();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
