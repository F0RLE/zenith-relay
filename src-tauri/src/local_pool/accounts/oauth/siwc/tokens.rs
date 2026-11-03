use super::{OAuthError, OAuthErrorCode, SiwcRegistration, timing};
use crate::local_pool::accounts::oauth::{OAuthTokenSet, parse::validate_token};
use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    earliest_refresh_at: Option<serde_json::Value>,
}

pub(super) fn parse(
    body: &[u8],
    registration: &mut SiwcRegistration,
    refreshing: bool,
    now_ms: u64,
) -> Result<OAuthTokenSet, OAuthError> {
    let invalid = || OAuthError::new(OAuthErrorCode::InvalidResponse, false);
    let response: Response = serde_json::from_slice(body).map_err(|_| invalid())?;
    let scopes = match response.scope {
        Some(scope) => scope.split_whitespace().map(str::to_string).collect(),
        None if refreshing => registration.scopes.clone(),
        None => return Err(invalid()),
    };
    for token in [
        &response.access_token,
        &response.refresh_token,
        &response.id_token,
    ]
    .into_iter()
    .flatten()
    {
        validate_token(token)?;
    }
    let offline = scopes.iter().any(|scope| scope == "offline_access");
    let direct = scopes
        .iter()
        .any(|scope| scope == "chatgpt.tokens.use.direct");
    let expires_at_ms = if response.access_token.is_some() {
        if !response
            .token_type
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("Bearer"))
            || (refreshing || offline) && response.refresh_token.is_none()
        {
            return Err(invalid());
        }
        Some(
            response
                .expires_in
                .filter(|seconds| *seconds > 0)
                .and_then(|seconds| seconds.checked_mul(1_000))
                .and_then(|milliseconds| now_ms.checked_add(milliseconds))
                .ok_or_else(invalid)?,
        )
    } else {
        if refreshing || offline || direct || response.refresh_token.is_some() {
            return Err(invalid());
        }
        None
    };
    if !refreshing && response.id_token.is_none() {
        return Err(invalid());
    }
    let earliest = timing::refresh_deadline_ms(response.earliest_refresh_at.as_ref())?;
    registration.scopes = scopes;
    registration.earliest_refresh_at_ms = earliest;
    Ok(OAuthTokenSet {
        // The existing credential wire represents absent OAuth material with an
        // empty access token. Only the validated identity-only constructor accepts it.
        access_token: response.access_token.unwrap_or_default(),
        refresh_token: response.refresh_token,
        id_token: response.id_token,
        expires_at_ms,
        verified_identity: None,
        siwc: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn siwc_identity_only_retains_identity_without_authorization_material() {
        let mut registration = super::super::tests::registration();
        let body = json!({"scope":"openid profile email", "id_token":"synthetic-id"});
        let tokens = parse(
            &serde_json::to_vec(&body).unwrap(),
            &mut registration,
            false,
            1000,
        )
        .unwrap();
        assert!(tokens.access_token().is_empty());
        assert!(tokens.refresh_token().is_none());
        assert_eq!(tokens.id_token(), Some("synthetic-id"));
        assert_eq!(tokens.expires_at_ms(), None);
        assert!(!registration.plan_use_enabled());
    }

    #[test]
    fn siwc_refresh_retains_omitted_scopes_and_parses_date_deadline() {
        let mut registration = super::super::tests::registration();
        let before = registration.scopes.clone();
        let body = json!({"access_token":"next-access","refresh_token":"next-refresh",
            "token_type":"Bearer","expires_in":3600,"earliest_refresh_at":"2026-11-01T00:00:00Z"});
        let tokens = parse(
            &serde_json::to_vec(&body).unwrap(),
            &mut registration,
            true,
            1000,
        )
        .unwrap();
        assert_eq!(registration.scopes, before);
        assert_eq!(registration.earliest_refresh_at_ms, Some(1_793_491_200_000));
        assert_eq!(tokens.expires_at_ms(), Some(3_601_000));
    }

    #[test]
    fn siwc_rejects_incomplete_grants_and_overflow_without_mutating_registration() {
        for (body, refreshing) in [
            (
                json!({"scope":"openid offline_access","id_token":"id"}),
                false,
            ),
            (
                json!({"scope":"openid chatgpt.tokens.use.direct","id_token":"id"}),
                false,
            ),
            (json!({"scope":"openid","id_token":"id"}), true),
            (
                json!({"scope":"openid","access_token":"","id_token":"id"}),
                false,
            ),
            (
                json!({"scope":"openid","refresh_token":"refresh","id_token":"id"}),
                false,
            ),
            (
                json!({"scope":"openid","access_token":"access","id_token":"id","token_type":"Bearer","expires_in":u64::MAX}),
                false,
            ),
            (json!({"id_token":"id"}), false),
        ] {
            let mut registration = super::super::tests::registration();
            let before = registration.clone();
            assert!(
                parse(
                    &serde_json::to_vec(&body).unwrap(),
                    &mut registration,
                    refreshing,
                    1000
                )
                .is_err()
            );
            assert_eq!(registration, before);
        }
    }
}
