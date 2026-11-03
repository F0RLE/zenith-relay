use super::{LEGACY_PROVIDER_ID, PROVIDER_ID};
use toml_edit::{DocumentMut, Item};

pub(super) fn config_uses_zenith_provider(config_text: &str) -> bool {
    config_text
        .parse::<DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("model_provider")
                .and_then(Item::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|provider_id| provider_id == PROVIDER_ID || provider_id == LEGACY_PROVIDER_ID)
}

pub(super) fn is_zenith_customer_key(key: &str) -> bool {
    key.starts_with("znt_")
}
