use zenith_relay_core::providers::chatgpt::{ModelDiscoveryFailure, OAuthClientKind};

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
