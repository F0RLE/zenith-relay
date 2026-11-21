use crate::{Error, Result, is_valid_model_id};

/// Native Codex Images routes supported by Relay, not account entitlement evidence.
/// Keep the default first; explicit model selections are never substituted.
pub const CODEX_IMAGE_MODELS: &[&str] = &[
    "gpt-image-2.6-sunburst",
    "gpt-image-2.6-flare",
    "gpt-image-2.5-sunburst",
    "gpt-image-2.5-flare",
];

/// Default for an Images request that omits its model.
pub const IMAGE_API_MODEL: &str = CODEX_IMAGE_MODELS[0];

/// Re-enable only after the upstream Excel/BPS image service is verified.
/// The adapter remains implemented; this switch controls catalog and dispatch.
pub const BPS_IMAGES_ENABLED: bool = cfg!(feature = "experimental-bps-images");

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageGenerationRestriction {
    ProviderFreePlan,
    ProviderUnavailable,
}

pub fn account_image_restriction(
    kind: crate::providers::chatgpt::OAuthClientKind,
    plan_type: Option<&str>,
) -> Option<ImageGenerationRestriction> {
    use crate::providers::chatgpt::OAuthClientKind;
    let free = crate::quota::Subscription {
        plan_type: plan_type.map(str::to_owned),
        ..Default::default()
    }
    .is_free_plan();
    if kind == OAuthClientKind::Codex && free {
        Some(ImageGenerationRestriction::ProviderFreePlan)
    } else if kind == OAuthClientKind::ExcelBps && !BPS_IMAGES_ENABLED {
        Some(ImageGenerationRestriction::ProviderUnavailable)
    } else {
        None
    }
}

/// Provider restriction for the native Codex image route. Unknown plans do not
/// imply Free, and this does not restrict text or another client's transport.
pub fn account_images_available(
    kind: crate::providers::chatgpt::OAuthClientKind,
    plan_type: Option<&str>,
) -> bool {
    supports_account_images(kind) && account_image_restriction(kind, plan_type).is_none()
}

/// Keep text inventory intact while excluding unavailable account image routes,
/// including IDs restored from older discovery snapshots.
pub fn account_model_enabled(
    kind: crate::providers::chatgpt::OAuthClientKind,
    model: &str,
    plan_type: Option<&str>,
) -> bool {
    !matches!(
        kind,
        crate::providers::chatgpt::OAuthClientKind::ExcelBps
            | crate::providers::chatgpt::OAuthClientKind::Codex
    ) || account_images_available(kind, plan_type)
        || !is_image_model_id(model)
}

/// Advertised account image IDs, not an allowlist or account entitlement proof.
pub fn account_image_models(
    kind: crate::providers::chatgpt::OAuthClientKind,
) -> impl Iterator<Item = &'static str> {
    use crate::providers::chatgpt::OAuthClientKind;
    CODEX_IMAGE_MODELS
        .iter()
        .copied()
        .filter(move |_| supports_account_images(kind))
        .chain((kind == OAuthClientKind::ExcelBps && BPS_IMAGES_ENABLED).then_some("gpt-image-2"))
}

pub(crate) fn supports_account_images(kind: crate::providers::chatgpt::OAuthClientKind) -> bool {
    use crate::providers::chatgpt::OAuthClientKind;
    kind == OAuthClientKind::Codex || (kind == OAuthClientKind::ExcelBps && BPS_IMAGES_ENABLED)
}

pub fn is_image_model_id(model: &str) -> bool {
    let model_id = crate::model_id_key(model);
    model_id.starts_with("gpt-image-") || model_id.starts_with("dall-e-")
}

impl super::GatewayRuntime {
    pub(crate) fn unavailable_account_images(
        &self,
        key: &super::AuthenticatedKey,
        requested: &str,
        protocols: &[crate::WireApi],
    ) -> Option<ImageGenerationRestriction> {
        let model = self.resolve_model(key, requested)?;
        if !crate::is_valid_model_id(&model) || !is_image_model_id(&model) {
            return None;
        }
        let scope = key.scope_snapshot();
        // Credential/plan guards precede the scheduler lock during updates.
        let restrictions = self
            .account_state
            .executors
            .iter()
            .filter_map(|(id, account)| {
                let plan = crate::poison::read(&account.subscription_plan_type);
                account_image_restriction(account.oauth_client_kind, plan.as_deref())
                    .map(|restriction| (id, restriction))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        self.lock_scheduler().candidates().find_map(|candidate| {
            if !candidate.is_discovery_visible(&scope)
                || !candidate.supports_protocol(&model, protocols)
                || !candidate.model_rules.allows(&model)
                || !scope.model_rules.allows(&model)
            {
                return None;
            }
            restrictions.get(&candidate.id).copied()
        })
    }

    pub(crate) fn resolve_image_model(
        &self,
        key: &super::AuthenticatedKey,
        requested: &str,
        protocols: &[crate::WireApi],
        now_ms: u64,
    ) -> Option<String> {
        if let Some(model) = self.resolve_visible_model(key, requested, protocols, now_ms) {
            return Some(model);
        }
        let model = self.resolve_model(key, requested)?;
        if !crate::is_valid_model_id(&model) || !is_image_model_id(&model) {
            return None;
        }
        let scope = key.scope_snapshot();
        self.lock_scheduler()
            .candidates()
            .any(|candidate| {
                candidate.native_images && candidate.is_catalog_visible(&model, protocols, &scope)
            })
            .then_some(model)
    }
}

/// Preserve import compatibility; native Images no longer selects a text model.
pub fn normalize_image_base_model(model_id: Option<String>) -> Result<Option<String>> {
    let Some(model_id) = model_id else {
        return Ok(None);
    };
    let model_id = model_id.trim();
    if model_id.is_empty() || model_id.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if !is_valid_model_id(model_id) {
        return Err(Error::Validation(
            "image base model id is invalid".to_string(),
        ));
    }
    Ok(Some(model_id.to_string()))
}
