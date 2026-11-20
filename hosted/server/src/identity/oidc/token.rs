use super::Failure;
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    exp: u64,
    iat: i64,
    nbf: Option<u64>,
    nonce: String,
    azp: Option<String>,
}

pub(super) fn subject(
    token: &str,
    keys: &JwkSet,
    issuer: &str,
    client: &str,
    nonce: &str,
) -> Result<String, Failure> {
    use Failure::Denied;
    let header = decode_header(token).map_err(|_| Denied)?;
    if header.alg != Algorithm::RS256 || header.crit.as_ref().is_some_and(|v| !v.is_empty()) {
        return Err(Denied);
    }
    let mut candidates = keys.keys.iter().filter(|key| {
        let common = &key.common;
        matches!(key.algorithm, AlgorithmParameters::RSA(_))
            && common
                .public_key_use
                .as_ref()
                .is_none_or(|v| *v == PublicKeyUse::Signature)
            && common
                .key_operations
                .as_ref()
                .is_none_or(|ops| ops.contains(&KeyOperations::Verify))
            && common
                .key_algorithm
                .is_none_or(|alg| alg == KeyAlgorithm::RS256)
            && header
                .kid
                .as_ref()
                .is_none_or(|kid| common.key_id.as_ref() == Some(kid))
    });
    let key = candidates.next().ok_or(Denied)?;
    if candidates.next().is_some() {
        return Err(Denied);
    }
    let key = DecodingKey::from_jwk(key).map_err(|_| Denied)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_required_spec_claims(&["iss", "sub", "aud", "exp"]);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[client]);
    validation.leeway = 0;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(token, &key, &validation)
        .map_err(|_| Denied)?
        .claims;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Failure::Unavailable)?
        .as_secs();
    let audience = match &claims.aud {
        Audience::One(value) => value == client,
        Audience::Many(values) => values.len() == 1 && values[0] == client,
    };
    if claims.iss != issuer
        || !audience
        || claims.exp <= now
        || claims.azp.as_deref().is_some_and(|azp| azp != client)
        || !(now as i64 - 210..=now as i64 + 30).contains(&claims.iat)
        || claims.nbf.is_some_and(|nbf| nbf > now)
        || !bool::from(claims.nonce.as_bytes().ct_eq(nonce.as_bytes()))
        || !(1..=255).contains(&claims.sub.len())
        || !claims
            .sub
            .bytes()
            .all(|b| b.is_ascii() && !b.is_ascii_control())
    {
        return Err(Denied);
    }
    Ok(claims.sub)
}

#[cfg(test)]
mod tests;
