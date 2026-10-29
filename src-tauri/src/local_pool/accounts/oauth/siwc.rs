//! A SIWC registration is not a Codex account/workspace claim.
use super::error::{OAuthError, OAuthErrorCode};
use serde::{Deserialize, Serialize};
pub use zenith_relay_core::providers::chatgpt::SiwcRegistration;
pub(super) use zenith_relay_core::providers::chatgpt::valid_siwc_client_id as valid_client_id;

pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const ISSUER: &str = "https://auth.openai.com";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SiwcPending {
    pub client_id: String,
    pub host_id: String,
    pub subject: Option<String>,
    pub nonce: String,
    #[serde(default)]
    pub consent: bool,
}

impl super::client::CodexOAuthClient {
    pub async fn revoke_siwc(
        &self,
        registration: &SiwcRegistration,
        refresh_token: &str,
    ) -> Result<(), OAuthError> {
        registration
            .validate()
            .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
        super::parse::validate_token(refresh_token)?;
        let invalid = || OAuthError::new(OAuthErrorCode::TokenEndpointRejected, true);
        let gate = zenith_relay_core::scheduler::refresh::http::management_http_gate();
        let class = zenith_relay_core::scheduler::refresh::http::HttpClass::Auth;
        let (response, permit) = gate
            .send(
                &self.http,
                self.http
                    .get(format!("{ISSUER}/.well-known/openid-configuration")),
                class,
            )
            .await
            .map_err(|_| invalid())?;
        if !response.status().is_success() {
            return Err(invalid());
        }
        let bytes = super::super::collect_limited(response, super::MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|_| invalid())?;
        drop(permit);
        #[derive(Deserialize)]
        struct Discovery {
            issuer: String,
            revocation_endpoint: String,
        }
        let discovery: Discovery = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if discovery.issuer != ISSUER
            || discovery.revocation_endpoint != format!("{ISSUER}/api/accounts/oauth/revoke")
        {
            return Err(invalid());
        }
        let request = self.http.post(discovery.revocation_endpoint).form(&[
            ("client_id", registration.client_id.as_str()),
            ("token", refresh_token),
            ("token_type_hint", "refresh_token"),
        ]);
        let (response, permit) = gate
            .send(&self.http, request, class)
            .await
            .map_err(|_| invalid())?;
        let status = response.status();
        let _ = super::super::collect_limited(response, super::MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|_| invalid())?;
        drop(permit);
        if status.is_success() {
            Ok(())
        } else {
            Err(invalid())
        }
    }

    pub fn with_siwc_registration(
        mut self,
        host_id: String,
        registration: Option<&SiwcRegistration>,
    ) -> Result<Self, OAuthError> {
        if let Some(registration) = registration {
            registration
                .validate()
                .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
            if registration.host_id != host_id && !registration.requires_reauthentication {
                return Err(OAuthError::new(OAuthErrorCode::InvalidConfiguration, false));
            }
        }
        self.siwc = Some(SiwcPending {
            client_id: registration.map_or_else(
                || zenith_relay_core::providers::chatgpt::SIWC_BOOTSTRAP_CLIENT_ID.to_string(),
                |r| r.client_id.clone(),
            ),
            host_id,
            subject: registration.map(|r| r.subject.clone()),
            nonce: String::new(),
            consent: registration.is_some_and(|r| !r.plan_use_enabled()),
        });
        Ok(self)
    }

    pub(super) async fn validate_siwc_tokens(
        &self,
        body: &[u8],
        mut registration: SiwcRegistration,
        nonce: Option<&str>,
        now_ms: u64,
    ) -> Result<super::OAuthTokenSet, OAuthError> {
        let invalid = || OAuthError::new(OAuthErrorCode::InvalidResponse, false);
        #[derive(Deserialize)]
        struct Response {
            token_type: String,
            scope: Option<String>,
            expires_in: u64,
            earliest_refresh_at: Option<u64>,
        }
        let response: Response = serde_json::from_slice(body).map_err(|_| invalid())?;
        if !response.token_type.eq_ignore_ascii_case("Bearer") || response.expires_in == 0 {
            return Err(invalid());
        }
        let mut tokens = super::parse::parse_token_response(body, now_ms)?;
        if tokens.refresh_token().is_none() {
            return Err(invalid());
        }
        if let Some(scope) = response.scope {
            registration.scopes = scope.split_whitespace().map(str::to_string).collect();
        } else if nonce.is_some() {
            return Err(invalid());
        }
        registration.earliest_refresh_at_ms = response
            .earliest_refresh_at
            .map(|seconds| seconds.saturating_mul(1_000));
        if let Some(id_token) = tokens.id_token() {
            let (response, permit) =
                zenith_relay_core::scheduler::refresh::http::management_http_gate()
                    .send(
                        &self.http,
                        self.http.get(format!("{ISSUER}/.well-known/jwks.json")),
                        zenith_relay_core::scheduler::refresh::http::HttpClass::Auth,
                    )
                    .await
                    .map_err(|_| OAuthError::new(OAuthErrorCode::Transport, true))?;
            if !response.status().is_success() {
                return Err(invalid());
            }
            let bytes = super::super::collect_limited(response, super::MAX_TOKEN_RESPONSE_BYTES)
                .await
                .map_err(|_| invalid())?;
            drop(permit);
            let jwks: jsonwebtoken::jwk::JwkSet =
                serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            let claims = verify_id_token(id_token, &jwks, &registration.client_id, nonce, now_ms)?;
            if !registration.subject.is_empty() && registration.subject != claims.sub {
                return Err(invalid());
            }
            registration.subject = claims.sub.clone();
            tokens.verified_identity = Some(super::identity::OAuthIdentityClaims {
                email: claims.email,
                plan_type: claims.plan_type,
                subscription_active_until_ms: claims.subscription_active_until_ms,
                user_id: Some(claims.sub),
                account_id: Some(registration.identity_key()),
                account_is_fedramp: false,
                expires_at_ms: tokens.expires_at_ms(),
            });
        } else if nonce.is_some() || registration.subject.is_empty() {
            return Err(invalid());
        }
        registration.requires_reauthentication = false;
        registration.validate().map_err(|_| invalid())?;
        tokens.siwc = Some(registration);
        Ok(tokens)
    }

    pub async fn exchange_siwc_refresh(
        &self,
        registration: &SiwcRegistration,
        refresh_token: &str,
        now_ms: u64,
    ) -> Result<super::OAuthTokenSet, zenith_relay_core::accounts::TokenRefreshFailure> {
        use zenith_relay_core::accounts::{
            TokenRefreshFailure as Failure, TokenRefreshFailureKind as Kind,
        };
        use zenith_relay_core::providers::chatgpt::{
            token_refresh_failure_kind, token_refresh_provider_error_code,
        };
        let invalid = || Failure::new(Kind::Transient, "invalid_siwc_refresh_response");
        registration.validate().map_err(|_| invalid())?;
        if registration.requires_reauthentication {
            return Err(Failure::new(
                Kind::Transient,
                "siwc_reauthentication_required",
            ));
        }
        super::parse::validate_token(refresh_token).map_err(|_| invalid())?;
        if registration
            .earliest_refresh_at_ms
            .is_some_and(|at| now_ms < at)
        {
            return Err(Failure::new(Kind::Transient, "siwc_refresh_too_early"));
        }
        let request = self
            .http
            .post(format!("{ISSUER}/api/accounts/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", registration.client_id.as_str()),
                ("refresh_token", refresh_token),
                ("resource", RESOURCE),
            ]);
        let (response, permit) =
            zenith_relay_core::scheduler::refresh::http::management_http_gate()
                .send(
                    &self.http,
                    request,
                    zenith_relay_core::scheduler::refresh::http::HttpClass::Auth,
                )
                .await
                .map_err(|_| Failure::new(Kind::Transient, "transport"))?;
        let status = response.status();
        let bytes = super::super::collect_limited(response, super::MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|_| invalid())?;
        drop(permit);
        if !status.is_success() {
            let code = token_refresh_provider_error_code(&bytes)
                .unwrap_or_else(|| "token_refresh_failed".into());
            let kind = match code.as_str() {
                "invalid_grant" => Kind::InvalidGrant,
                "invalid_refresh_token" | "token_expired" | "refresh_token_expired" => {
                    Kind::ExpiredRefreshToken
                }
                "refresh_token_invalidated" | "refresh_token_reused" => {
                    Kind::InvalidatedRefreshToken
                }
                _ => token_refresh_failure_kind(&code),
            };
            return Err(Failure::new(kind, &code));
        }
        self.validate_siwc_tokens(&bytes, registration.clone(), None, now_ms)
            .await
            .map_err(|_| invalid())
    }
}

#[derive(Clone, Deserialize)]
pub(super) struct VerifiedClaims {
    sub: String,
    aud: serde_json::Value,
    azp: Option<String>,
    email: Option<String>,
    nonce: Option<String>,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    #[serde(skip)]
    plan_type: Option<String>,
    #[serde(skip)]
    subscription_active_until_ms: Option<u64>,
}

fn verify_id_token(
    token: &str,
    jwks: &jsonwebtoken::jwk::JwkSet,
    client_id: &str,
    nonce: Option<&str>,
    now_ms: u64,
) -> Result<VerifiedClaims, OAuthError> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
    let invalid = || OAuthError::new(OAuthErrorCode::InvalidJwt, false);
    let header = decode_header(token).map_err(|_| invalid())?;
    if header.alg != Algorithm::RS256 {
        return Err(invalid());
    }
    let kid = header.kid.ok_or_else(invalid)?;
    let key = jwks.find(&kid).ok_or_else(invalid)?;
    let key = DecodingKey::from_jwk(key).map_err(|_| invalid())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "sub"]);
    validation.validate_nbf = true;
    validation.leeway = 0;
    let mut claims = decode::<VerifiedClaims>(token, &key, &validation)
        .map_err(|_| invalid())?
        .claims;
    if claims.azp.as_deref().is_some_and(|azp| azp != client_id)
        || (claims.aud.as_array().is_some_and(|aud| aud.len() > 1)
            && claims.azp.as_deref() != Some(client_id))
        || claims.sub.is_empty()
        || claims.sub.len() > 256
        || claims.sub.chars().any(char::is_control)
        || now_ms / 1000 >= claims.exp
        || claims.nbf.is_some_and(|nbf| now_ms / 1000 < nbf)
        || nonce.is_some_and(|nonce| claims.nonce.as_deref() != Some(nonce))
    {
        return Err(invalid());
    }
    if let Ok(metadata) = super::parse::parse_identity_claims(token) {
        claims.plan_type = metadata.plan_type;
        claims.subscription_active_until_ms = metadata.subscription_active_until_ms;
    }
    Ok(claims)
}

#[cfg(test)]
mod tests;
