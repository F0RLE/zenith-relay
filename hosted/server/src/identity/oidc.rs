use openidconnect::{
    AdditionalClaims, ClientId, IdToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    core::{
        CoreGenderClaim, CoreIdTokenVerifier, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    },
};
use reqwest::{Client, Response, header};
use ring::hmac;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::{Host, Url};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcSettings {
    pub id: Uuid,
    pub issuer: String,
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

impl OidcSettings {
    pub(crate) fn validate(&self, site: &Url) -> Result<(), &'static str> {
        let bad = "invalid customer OIDC registration";
        let parse = |s: &str| {
            let u = Url::parse(s).map_err(|_| bad)?;
            let loopback = matches!(u.host(), Some(Host::Ipv4(ip)) if ip.is_loopback())
                || matches!(u.host(), Some(Host::Ipv6(ip)) if ip.is_loopback());
            if !(u.scheme() == "https"
                || u.scheme() == "http" && site.scheme() == "http" && loopback)
                || !u.has_host()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
                || s.len() > 2048
                || s.bytes().any(|c| !c.is_ascii_graphic() || c == b'\\')
                || (u.as_str() != s && u.origin().ascii_serialization() != s)
            {
                return Err(bad);
            }
            Ok(u)
        };
        let iss = parse(&self.issuer)?;
        let endpoints = [
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.jwks_uri,
        ];
        if self.id.is_nil()
            || !(1..=256).contains(&self.client_id.len())
            || !self.client_id.bytes().all(|b| b.is_ascii_graphic())
            || iss.origin() == site.origin()
            || endpoints[0] == endpoints[1]
            || endpoints[1] == endpoints[2]
            || endpoints[0] == endpoints[2]
        {
            return Err(bad);
        }
        for endpoint in endpoints {
            if parse(endpoint)?.origin() != iss.origin() {
                return Err(bad);
            }
        }
        Ok(())
    }
}

pub(super) struct Provider {
    pub settings: OidcSettings,
    redirect: String,
    pub binding: [u8; 32],
    digest_key: hmac::Key,
    http: Client,
}

#[derive(Debug, Deserialize, Serialize)]
struct Validity {
    nbf: Option<i64>,
}
impl AdditionalClaims for Validity {}
type CustomerToken =
    IdToken<Validity, CoreGenderClaim, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm>;

fn tag(key: &hmac::Key, fields: &[&[u8]]) -> [u8; 32] {
    let mut ctx = hmac::Context::with_key(key);
    for f in fields {
        ctx.update(&(f.len() as u64).to_be_bytes());
        ctx.update(f);
    }
    ctx.sign().as_ref().try_into().expect("SHA-256")
}

impl Provider {
    pub fn new(s: OidcSettings, origin: &str, secret: &[u8; 32]) -> Result<Self, &'static str> {
        let redirect = format!("{origin}/api/hosted/v1/auth/customer/{}/callback", s.id);
        let digest_key = hmac::Key::new(hmac::HMAC_SHA256, secret);
        let binding = tag(
            &digest_key,
            &[
                b"oidc-binding-v1",
                s.id.as_bytes(),
                s.issuer.as_bytes(),
                s.client_id.as_bytes(),
                s.authorization_endpoint.as_bytes(),
                s.token_endpoint.as_bytes(),
                s.jwks_uri.as_bytes(),
                redirect.as_bytes(),
            ],
        );
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "cannot configure customer identity transport")?;
        Ok(Self {
            settings: s,
            redirect,
            binding,
            digest_key,
            http,
        })
    }

    pub fn authorize(&self, state: &str, nonce: &Nonce) -> (Url, PkceCodeVerifier) {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut url =
            Url::parse(&self.settings.authorization_endpoint).expect("validated endpoint");
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.settings.client_id.as_str()),
            ("redirect_uri", &self.redirect),
            ("response_type", "code"),
            ("scope", "openid"),
            ("nonce", nonce.secret()),
            ("state", state),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ]);
        (url, verifier)
    }

    pub async fn subject(
        &self,
        code: &str,
        verifier: PkceCodeVerifier,
        nonce: Nonce,
    ) -> Result<[u8; 32], Failure> {
        use Failure::*;
        let s = &self.settings;
        let response = self
            .http
            .post(&s.token_endpoint)
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", &s.client_id),
                ("redirect_uri", &self.redirect),
                ("code_verifier", verifier.secret()),
            ])
            .send()
            .await
            .map_err(|_| Unavailable)?;
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(Denied);
        }
        #[derive(Deserialize)]
        struct Exchange {
            id_token: String,
        }
        let bytes = bounded_json(response, false).await?;
        let Exchange { id_token } = serde_json::from_slice(&bytes).map_err(|_| Denied)?;
        let id: CustomerToken = Zeroizing::new(id_token).parse().map_err(|_| Denied)?;
        let response = self
            .http
            .get(&s.jwks_uri)
            .header(header::ACCEPT, "application/json, application/jwk-set+json")
            .send()
            .await
            .map_err(|_| Unavailable)?;
        let keys: CoreJsonWebKeySet = serde_json::from_slice(&bounded_json(response, true).await?)
            .map_err(|_| Unavailable)?;
        if keys.keys().is_empty() || keys.keys().len() > 32 {
            return Err(Unavailable);
        }
        let checker = CoreIdTokenVerifier::new_public_client(
            ClientId::new(s.client_id.clone()),
            IssuerUrl::new(s.issuer.clone()).map_err(|_| Unavailable)?,
            keys,
        )
        .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256]);
        let claims = id.claims(&checker, &nonce).map_err(|_| Denied)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Unavailable)?
            .as_secs() as i64;
        let sub = claims.subject().as_str();
        if claims.issuer().as_str() != s.issuer
            || claims.audiences().len() != 1
            || claims
                .authorized_party()
                .is_some_and(|azp| azp.as_str() != s.client_id)
            || !(now - 210..=now + 30).contains(&claims.issue_time().timestamp())
            || claims.additional_claims().nbf.is_some_and(|t| t > now)
            || !(1..=255).contains(&sub.len())
            || !sub.bytes().all(|b| b.is_ascii() && !b.is_ascii_control())
        {
            return Err(Denied);
        }
        Ok(tag(
            &self.digest_key,
            &[
                b"oidc-subject-v1",
                s.id.as_bytes(),
                s.issuer.as_bytes(),
                s.client_id.as_bytes(),
                sub.as_bytes(),
            ],
        ))
    }
}

pub(super) enum Failure {
    Denied,
    Unavailable,
}

async fn bounded_json(mut response: Response, jwks: bool) -> Result<Zeroizing<Vec<u8>>, Failure> {
    let ct = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    let cap = if jwks { 65536 } else { 32768 };
    if !response.status().is_success()
        || !(ct == Some("application/json") || jwks && ct == Some("application/jwk-set+json"))
        || response.content_length().is_some_and(|n| n > cap as u64)
    {
        return Err(Failure::Unavailable);
    }
    let mut buf = Zeroizing::new(Vec::new());
    while let Some(part) = response.chunk().await.map_err(|_| Failure::Unavailable)? {
        if part.len() > cap - buf.len() {
            return Err(Failure::Unavailable);
        }
        buf.extend_from_slice(&part);
    }
    Ok(buf)
}
