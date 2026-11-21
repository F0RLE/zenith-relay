use zenith_relay_core::accounts::{TokenRefreshFailure, TokenRefreshFailureKind};

pub(super) fn record_renewal_failure(account_id: &str, failure: &TokenRefreshFailure) {
    if failure.kind == TokenRefreshFailureKind::Deferred && !crate::diagnostics::is_debug_enabled()
    {
        return;
    }
    let details = [
        ("account", crate::diagnostics::hash_identifier(account_id)),
        ("code", failure.code.clone()),
        (
            "stage",
            failure
                .stage
                .map_or("unknown", |stage| stage.as_str())
                .into(),
        ),
        (
            "request_id",
            failure
                .request_id
                .clone()
                .unwrap_or_else(|| "unreported".into()),
        ),
        (
            "next_retry_at_ms",
            failure
                .retry_at_ms
                .map_or_else(|| "unreported".into(), |at| at.to_string()),
        ),
    ];
    if failure.kind == TokenRefreshFailureKind::Deferred {
        crate::diagnostics::record_operation("siwc-token-renewal", "scheduled", &details);
    } else {
        crate::diagnostics::record_error(
            "siwc-token-renewal",
            Some(&failure.code),
            "SIWC token renewal could not finish",
            &details,
        );
    }
}
