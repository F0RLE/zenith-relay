use super::error::{OAuthError, OAuthErrorCode};
use super::exchange::OAuthTokenSet;
use super::identity::OAuthIdentityClaims;
use super::MAX_TOKEN_BYTES;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zenith_relay_core::accounts::decode_unverified_jwt_payload;
use zenith_relay_core::omit_blank;

#[derive(Serialize)]
pub(super) struct AuthorizationCodeRequest<'a> {
    pub(super) grant_type: &'static str,
    pub(super) code: &'a str,
    pub(super) redirect_uri: &'a str,
    pub(super) client_id: &'static str,
    pub(super) code_verifier: &'a str,
}

#[derive(Serialize)]
pub(super) struct RefreshTokenRequest<'a> {
    pub(super) client_id: &'static str,
    pub(super) grant_type: &'static str,
    pub(super) refresh_token: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct JwtClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
struct AuthClaims {
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_subscription_active_until: Option<Value>,
    #[serde(default)]
    chatgpt_user_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
}

pub(super) fn parse_token_response(body: &[u8], now_ms: u64) -> Result<OAuthTokenSet, OAuthError> {
    let response: TokenResponse = serde_json::from_slice(body)
        .map_err(|_| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
    let access_token = response
        .access_token
        .ok_or_else(|| OAuthError::new(OAuthErrorCode::InvalidResponse, false))?;
    validate_token(&access_token)?;
    validate_optional_token(response.refresh_token.as_deref())?;
    validate_optional_token(response.id_token.as_deref())?;
    let expires_at_ms = response
        .expires_in
        .map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1_000)))
        .or_else(|| jwt_expiration_ms(&access_token).ok().flatten());
    Ok(OAuthTokenSet {
        access_token,
        refresh_token: omit_blank(response.refresh_token),
        id_token: omit_blank(response.id_token),
        expires_at_ms,
    })
}

pub(super) fn parse_identity_claims(jwt: &str) -> Result<OAuthIdentityClaims, OAuthError> {
    let claims: JwtClaims = decode_jwt_payload(jwt)?;
    let email =
        omit_blank(claims.email).or_else(|| claims.profile.and_then(|profile| profile.email));
    let auth = claims.auth;
    Ok(OAuthIdentityClaims {
        email,
        plan_type: auth
            .as_ref()
            .and_then(|auth| omit_blank(auth.chatgpt_plan_type.clone())),
        subscription_active_until_ms: auth
            .as_ref()
            .and_then(|auth| auth.chatgpt_subscription_active_until.as_ref())
            .and_then(zenith_relay_core::providers::chatgpt::parse_subscription_timestamp_ms),
        user_id: auth.as_ref().and_then(|auth| {
            omit_blank(auth.chatgpt_user_id.clone()).or_else(|| omit_blank(auth.user_id.clone()))
        }),
        account_id: auth
            .as_ref()
            .and_then(|auth| omit_blank(auth.chatgpt_account_id.clone())),
        account_is_fedramp: auth
            .as_ref()
            .is_some_and(|auth| auth.chatgpt_account_is_fedramp),
        expires_at_ms: claims.exp.map(|seconds| seconds.saturating_mul(1_000)),
    })
}

fn jwt_expiration_ms(jwt: &str) -> Result<Option<u64>, OAuthError> {
    let claims: JwtClaims = decode_jwt_payload(jwt)?;
    Ok(claims.exp.map(|seconds| seconds.saturating_mul(1_000)))
}

fn decode_jwt_payload<T: for<'de> Deserialize<'de>>(jwt: &str) -> Result<T, OAuthError> {
    decode_unverified_jwt_payload(jwt)
        .ok_or_else(|| OAuthError::new(OAuthErrorCode::InvalidJwt, false))
}

pub(super) fn validate_token(token: &str) -> Result<(), OAuthError> {
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(OAuthError::new(OAuthErrorCode::InvalidResponse, false))
    } else {
        Ok(())
    }
}

fn validate_optional_token(token: Option<&str>) -> Result<(), OAuthError> {
    match token {
        Some(token) => validate_token(token),
        None => Ok(()),
    }
}

pub(super) fn set_once(slot: &mut Option<String>, value: String) -> Result<(), OAuthError> {
    if slot.replace(value).is_some() {
        Err(OAuthError::new(OAuthErrorCode::InvalidCallback, false))
    } else {
        Ok(())
    }
}
