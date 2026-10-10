use super::{PricingCatalog, PricingError, CACHE_FORMAT, CACHE_SCHEMA_VERSION, LITELLM_SOURCE_URL};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CACHE_RECORDS: usize = 100_000;
pub const MAX_CACHE_STRING_LENGTH: usize = 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingCacheEnvelope {
    pub format: String,
    pub schema_version: u32,
    pub source_url: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    pub fetched_at_ms: u64,
    pub payload_sha256: String,
    #[serde(default)]
    pub stale: bool,
    pub payload: Value,
}

impl PricingCacheEnvelope {
    pub fn new(
        pricing_payload: Value,
        revision: String,
        fetched_at_ms: u64,
    ) -> Result<Self, PricingError> {
        let payload_sha256 = payload_hash(&pricing_payload)?;
        let envelope = Self {
            format: CACHE_FORMAT.to_string(),
            schema_version: CACHE_SCHEMA_VERSION,
            source_url: LITELLM_SOURCE_URL.to_string(),
            revision,
            etag: None,
            last_modified: None,
            fetched_at_ms,
            payload_sha256,
            stale: false,
            payload: pricing_payload,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), PricingError> {
        if self.format != CACHE_FORMAT
            || self.schema_version != CACHE_SCHEMA_VERSION
            || self.source_url != LITELLM_SOURCE_URL
            || self.revision.trim().is_empty()
            || self.fetched_at_ms == 0
            || self.revision.len() > MAX_CACHE_STRING_LENGTH
            || self.source_url.len() > MAX_CACHE_STRING_LENGTH
            || self
                .etag
                .as_ref()
                .is_some_and(|etag_value| etag_value.len() > MAX_CACHE_STRING_LENGTH)
            || self
                .last_modified
                .as_ref()
                .is_some_and(|last_modified_value| {
                    last_modified_value.len() > MAX_CACHE_STRING_LENGTH
                })
        {
            return Err(PricingError::InvalidCache);
        }
        let encoded = serde_json::to_vec(&self.payload).map_err(|_| PricingError::InvalidCache)?;
        if encoded.len() > MAX_CACHE_BYTES {
            return Err(PricingError::CacheTooLarge);
        }
        if !self.payload.is_object() {
            return Err(PricingError::InvalidCache);
        }
        let pricing_records = self.payload.as_object().ok_or(PricingError::InvalidCache)?;
        if pricing_records.len() > MAX_CACHE_RECORDS {
            return Err(PricingError::CacheTooLarge);
        }
        if pricing_records
            .keys()
            .any(|key| key.is_empty() || key.len() > MAX_CACHE_STRING_LENGTH)
        {
            return Err(PricingError::InvalidCache);
        }
        if payload_hash(&self.payload)? != self.payload_sha256 {
            return Err(PricingError::InvalidCache);
        }
        Ok(())
    }

    pub fn catalog(&self) -> Result<PricingCatalog, PricingError> {
        self.validate()?;
        PricingCatalog::from_litellm_payload(
            &self.payload,
            Some(self.revision.clone()),
            Some(self.fetched_at_ms),
            self.stale,
        )
    }
}

pub fn payload_hash(pricing_payload: &Value) -> Result<String, PricingError> {
    let serialized_payload =
        serde_json::to_vec(pricing_payload).map_err(|_| PricingError::InvalidCache)?;
    let digest = Sha256::digest(serialized_payload);
    Ok(format!("sha256:{}", hex::encode(digest)))
}

/// Returns a deterministic object containing only LiteLLM model records.
pub fn validate_litellm_payload(
    pricing_payload: &Value,
) -> Result<&serde_json::Map<String, Value>, PricingError> {
    let pricing_records = pricing_payload
        .as_object()
        .ok_or(PricingError::InvalidCatalog)?;
    if pricing_records.len() > MAX_CACHE_RECORDS {
        return Err(PricingError::CacheTooLarge);
    }
    if pricing_records
        .keys()
        .any(|key| key.is_empty() || key.len() > MAX_CACHE_STRING_LENGTH)
    {
        return Err(PricingError::InvalidCatalog);
    }
    Ok(pricing_records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_envelope_round_trips_and_detects_tampering() {
        let payload = json!({"gpt-test": {"input_cost_per_token": 0.000001}});
        let envelope = PricingCacheEnvelope::new(payload, "sha256:fixture".into(), 1).unwrap();
        assert!(envelope.validate().is_ok());
        let mut tampered = envelope.clone();
        tampered.payload_sha256 = "sha256:wrong".into();
        assert_eq!(tampered.validate(), Err(PricingError::InvalidCache));
    }

    #[test]
    fn rejects_non_object_and_oversized_metadata() {
        let payload = json!([]);
        assert_eq!(
            PricingCacheEnvelope::new(payload, "revision".into(), 1),
            Err(PricingError::InvalidCache)
        );
    }
}
