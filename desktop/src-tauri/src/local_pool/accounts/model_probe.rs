use zenith_relay_core::providers::chatgpt::{
    BasisPointsCatalogDiagnostics, BasisPointsModelDiagnostic, ModelDiscoveryFailure,
    OAuthClientKind,
};

pub(in crate::local_pool) fn record_catalog(
    account_id: &str,
    via_proxy: bool,
    catalog: &BasisPointsCatalogDiagnostics,
) {
    if !crate::diagnostics::is_debug_enabled() {
        return;
    }
    let common = [
        ("account", crate::diagnostics::hash_identifier(account_id)),
        ("stage", "bps_access".into()),
        (
            "request_route",
            if via_proxy {
                "configured_proxy"
            } else {
                "client_default"
            }
            .into(),
        ),
    ];
    let mut summary = common.to_vec();
    summary.extend(catalog_summary(catalog));
    crate::diagnostics::record_operation("bps-model-catalog", "observed", &summary);
    for model in &catalog.models {
        let mut details = common.to_vec();
        details.extend(catalog_model_details(model));
        crate::diagnostics::record_operation("bps-model-catalog", model.outcome.as_str(), &details);
    }
}

fn catalog_summary(catalog: &BasisPointsCatalogDiagnostics) -> [(&'static str, String); 6] {
    [
        ("access_allowed", catalog.access_allowed.to_string()),
        ("received_models", catalog.received_models.to_string()),
        ("accepted_models", catalog.accepted_models.to_string()),
        ("excluded_models", catalog.excluded_models().to_string()),
        ("reported_models", catalog.models.len().to_string()),
        ("omitted_models", catalog.omitted_models().to_string()),
    ]
}

fn catalog_model_details(model: &BasisPointsModelDiagnostic) -> [(&'static str, String); 2] {
    [
        ("row", model.row.to_string()),
        (
            "model",
            model
                .model_id
                .clone()
                .unwrap_or_else(|| "unreported".into()),
        ),
    ]
}

/// Record this probe's transport configuration, not the account's later proxy
/// setting. A default client may use a system proxy, so do not label it direct.
/// The existing error log supplies the timestamp and retention policy.
/// Never include proxy URLs, tokens, account identities or provider response bodies.
pub(in crate::local_pool) fn record_failure(
    account_id: &str,
    kind: OAuthClientKind,
    via_proxy: bool,
    error: &ModelDiscoveryFailure,
) {
    crate::diagnostics::record_error(
        "account-model-probe",
        Some(error.code.management_code()),
        "account model access check failed",
        &failure_details(account_id, kind, via_proxy, error),
    );
}

fn failure_details(
    account_id: &str,
    kind: OAuthClientKind,
    via_proxy: bool,
    error: &ModelDiscoveryFailure,
) -> [(&'static str, String); 5] {
    [
        ("account", crate::diagnostics::hash_identifier(account_id)),
        (
            "stage",
            match kind {
                OAuthClientKind::ExcelBps => "bps_access",
                OAuthClientKind::Codex => "codex_models",
                OAuthClientKind::Siwc => "siwc_models",
            }
            .into(),
        ),
        (
            "request_route",
            if via_proxy {
                "configured_proxy"
            } else {
                "client_default"
            }
            .into(),
        ),
        (
            "http_status",
            error
                .http_status
                .map_or_else(|| "none".into(), |status| status.to_string()),
        ),
        ("retryable", error.retryable.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::providers::chatgpt::ModelDiscoveryFailureCode;

    #[test]
    fn successful_catalog_diagnostics_keep_counts_and_decisions_without_private_fields() {
        let (_, catalog) = zenith_relay_core::providers::chatgpt::parse_basis_points_model_access_with_diagnostics(
            br#"{"allowed":true,"model_catalog":{"models":[
                {"id":"gpt-synthetic-new","description":"synthetic-private-body"},
                {"id":"gpt-synthetic-picker","model_picker_enabled":false},
                {"id":"synthetic private identifier","label":"synthetic-private-label"}
            ]}}"#,
        ).unwrap();
        let summary = catalog_summary(&catalog);
        assert_eq!(summary[0], ("access_allowed", "true".into()));
        assert_eq!(summary[1], ("received_models", "3".into()));
        assert_eq!(summary[2], ("accepted_models", "1".into()));
        assert_eq!(summary[3], ("excluded_models", "2".into()));
        assert_eq!(summary[5], ("omitted_models", "0".into()));
        assert_eq!(catalog.models[1].outcome.as_str(), "picker_disabled");
        assert_eq!(
            catalog_model_details(&catalog.models[1])[1],
            ("model", "gpt-synthetic-picker".into())
        );
        assert_eq!(
            catalog_model_details(&catalog.models[2])[1],
            ("model", "unreported".into())
        );
        let details = catalog
            .models
            .iter()
            .map(catalog_model_details)
            .collect::<Vec<_>>();
        let text = format!("{summary:?} {details:?}");
        for private in [
            "synthetic-private-body",
            "synthetic-private-label",
            "synthetic private identifier",
        ] {
            assert!(!text.contains(private));
        }
    }

    #[test]
    fn failed_probe_diagnostics_keep_actual_route_and_safe_cause_only() {
        let error = ModelDiscoveryFailure {
            code: ModelDiscoveryFailureCode::Forbidden,
            retryable: false,
            http_status: Some(403),
            retry_after_ms: None,
        };
        let fields = failure_details("synthetic-account", OAuthClientKind::ExcelBps, true, &error);
        assert_ne!(fields[0].1, "synthetic-account");
        assert_eq!(fields[1], ("stage", "bps_access".into()));
        assert_eq!(fields[2], ("request_route", "configured_proxy".into()));
        assert_eq!(fields[3], ("http_status", "403".into()));
        assert_eq!(fields[4], ("retryable", "false".into()));
        let direct = failure_details("synthetic-account", OAuthClientKind::Codex, false, &error);
        assert_eq!(direct[1].1, "codex_models");
        assert_eq!(direct[2].1, "client_default");
    }
}
