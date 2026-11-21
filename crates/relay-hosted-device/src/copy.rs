//! Static source copies have their own single-operation proof, not the pairing bearer.
use super::{Destination, RESPONSE, decode, id};
use reqwest::{Client, StatusCode, header};
use ring::{
    digest::SHA256,
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{future::Future, time::Duration};
use url::{Host, Url};
use uuid::Uuid;
use zeroize::Zeroizing;

const INVALID: &str = "invalid_copy_review";
const NETWORK: &str = "copy_result_unconfirmed";
pub const MAX_REVIEW_CODE: usize = 4096;
pub const COPY_STATUS_PATH: &str = "/api/hosted/v1/transfer/status";
pub const COPY_COMMIT_PATH: &str = "/api/hosted/v1/transfer/commit";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CopyPlan {
    pub origin: String,
    pub installation_id: Uuid,
    pub device_id: Uuid,
    pub operation_id: Uuid,
    pub source_id: Uuid,
    pub label: String,
    pub endpoint: String,
    pub proxy: Option<CopyProxy>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CopyProxy {
    pub endpoint: String,
    pub authenticated: bool,
}

impl CopyProxy {
    /// Input is the runtime's normalized proxy URL. Only the origin is review metadata.
    pub fn from_route(raw: &str) -> Option<Self> {
        let url = Url::parse(raw).ok()?;
        let endpoint = url.origin().ascii_serialization();
        let authenticated = !url.username().is_empty();
        if !matches!(url.scheme(), "https" | "http")
            || url.port().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || raw.len() > 2048
            || !url.has_host()
            || authenticated != url.password().is_some_and(|p| !p.is_empty())
            || (!authenticated && raw != format!("{endpoint}/"))
            || raw != url.as_str()
        {
            return None;
        }
        Some(Self {
            endpoint,
            authenticated,
        })
    }

    fn valid(&self) -> bool {
        self.endpoint.len() <= 300
            && Self::from_route(&format!("{}/", self.endpoint))
                .is_some_and(|p| p.endpoint == self.endpoint)
    }
}

impl CopyPlan {
    pub fn valid(&self) -> bool {
        self.origin.len() <= 300
            && Url::parse(&self.origin).is_ok_and(|u| {
                u.scheme() == "https"
                    && u.has_host()
                    && u.origin().ascii_serialization() == self.origin
            })
            && [
                self.installation_id,
                self.device_id,
                self.operation_id,
                self.source_id,
            ]
            .iter()
            .all(|id| !id.is_nil())
            && self.label.trim() == self.label
            && (1..=96).contains(&self.label.len())
            && !self.label.chars().any(char::is_control)
            && self.endpoint.len() <= 304
            && Url::parse(&self.endpoint).is_ok_and(|u| {
                let loopback = match u.host() {
                    Some(Host::Ipv4(ip)) => ip.is_loopback(),
                    Some(Host::Ipv6(ip)) => ip.is_loopback(),
                    _ => false,
                };
                u.has_host()
                    && (u.scheme() == "https"
                        || u.scheme() == "http" && loopback && u.port().is_some())
                    && self.endpoint == format!("{}/v1", u.origin().ascii_serialization())
            })
            && self.endpoint != format!("{}/v1", self.origin)
            && self.proxy.as_ref().is_none_or(CopyProxy::valid)
    }

    fn matches(&self, target: &Destination) -> bool {
        self.origin == target.origin
            && self.installation_id == target.installation
            && self.device_id == target.device
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CopyReview {
    pub plan: CopyPlan,
    pub verifier: String,
    pub commitment: String,
}

impl CopyReview {
    pub fn from_code(raw: &str) -> Option<Self> {
        if raw.len() > MAX_REVIEW_CODE {
            return None;
        }
        let bytes = hex::decode(raw.strip_prefix("copy1.")?).ok()?;
        let review: Self = serde_json::from_slice(&bytes).ok()?;
        (review.plan.valid()
            && review.verifier_bytes().is_some()
            && review.commitment_bytes().is_some())
        .then_some(review)
    }

    pub fn code(&self) -> String {
        format!(
            "copy1.{}",
            hex::encode(serde_json::to_vec(self).expect("copy review serializes"))
        )
    }

    pub fn verifier_bytes(&self) -> Option<[u8; 32]> {
        decode(&self.verifier).map(|v| *v)
    }

    pub fn commitment_bytes(&self) -> Option<[u8; 32]> {
        decode(&self.commitment).map(|v| *v)
    }
}

pub struct CopyProof {
    pub installation_id: Uuid,
    pub device_id: Uuid,
    pub operation_id: Uuid,
    secret: Zeroizing<[u8; 32]>,
}

impl CopyProof {
    pub fn presented(raw: &str) -> Option<Self> {
        let mut parts = raw.strip_prefix("rht1.")?.split('.');
        let proof = Self {
            installation_id: id(parts.next()?)?,
            device_id: id(parts.next()?)?,
            operation_id: id(parts.next()?)?,
            secret: decode(parts.next()?)?,
        };
        parts.next().is_none().then_some(proof)
    }

    pub fn verifier(&self) -> [u8; 32] {
        let mut bind = ring::digest::Context::new(&SHA256);
        bind.update(b"relay-hosted-copy-proof-v1");
        for v in [self.installation_id, self.device_id, self.operation_id] {
            bind.update(v.as_bytes());
        }
        bind.update(self.secret.as_ref());
        bind.finish().as_ref().try_into().expect("SHA-256")
    }

    fn bearer(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(format!(
            "rht1.{}.{}.{}.",
            self.installation_id, self.device_id, self.operation_id
        ));
        out.push_str(&Zeroizing::new(hex::encode(self.secret.as_ref())));
        out
    }

    fn payload(
        &self,
        plan: &CopyPlan,
        provider: &str,
        proxy: Option<&str>,
    ) -> Option<Zeroizing<Vec<u8>>> {
        let route = match proxy {
            Some(url) => Some(CopyProxy::from_route(url)?),
            None => None,
        };
        if self.installation_id != plan.installation_id
            || self.device_id != plan.device_id
            || self.operation_id != plan.operation_id
            || !(1..=1024).contains(&provider.len())
            || !provider.bytes().all(|b| b.is_ascii_graphic())
            || route != plan.proxy
        {
            return None;
        }
        let mut payload = Zeroizing::new(b"relay-hosted-copy-payload-v1\n".to_vec());
        payload.extend(serde_json::to_vec(plan).ok()?);
        payload.push(b'\n');
        payload.extend_from_slice(provider.as_bytes());
        payload.push(b'\n');
        payload.extend_from_slice(proxy.unwrap_or_default().as_bytes());
        Some(payload)
    }

    fn key(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, self.secret.as_ref())
    }

    pub fn matches_payload(
        &self,
        plan: &CopyPlan,
        commitment: &[u8],
        provider: &str,
        proxy: Option<&str>,
    ) -> bool {
        self.payload(plan, provider, proxy)
            .is_some_and(|payload| hmac::verify(&self.key(), &payload, commitment).is_ok())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CopyState {
    AwaitingApproval,
    Approved,
    Expired,
    Committed,
    Conflict,
}

pub struct CopyKey {
    review: CopyReview,
    proof: CopyProof,
}

/// Read from protected local storage after the server has approved this operation.
pub struct CopyInput {
    pub plan: CopyPlan,
    pub provider: Zeroizing<String>,
    pub proxy: Option<Zeroizing<String>>,
}

impl CopyKey {
    pub fn create(
        target: &Destination,
        plan: CopyPlan,
        provider: &str,
        proxy: Option<&str>,
    ) -> Result<Self, &'static str> {
        if !plan.valid() || !plan.matches(target) {
            return Err(INVALID);
        }
        let mut secret = Zeroizing::new([0; 32]);
        SystemRandom::new()
            .fill(secret.as_mut())
            .map_err(|_| "device_randomness_unavailable")?;
        let proof = CopyProof {
            installation_id: plan.installation_id,
            device_id: plan.device_id,
            operation_id: plan.operation_id,
            secret,
        };
        let payload = proof.payload(&plan, provider, proxy).ok_or(INVALID)?;
        let commitment = hex::encode(hmac::sign(&proof.key(), &payload).as_ref());
        let review = CopyReview {
            plan,
            commitment,
            verifier: hex::encode(proof.verifier()),
        };
        if review.code().len() > MAX_REVIEW_CODE {
            return Err(INVALID);
        }
        Ok(Self { review, proof })
    }

    pub fn review(&self) -> &CopyReview {
        &self.review
    }

    pub fn belongs_to(&self, target: &Destination) -> bool {
        self.review.plan.matches(target)
    }

    pub fn protected_record(&self) -> Zeroizing<String> {
        let mut out = Zeroizing::new(self.review.code());
        out.push('\n');
        out.push_str(&self.proof.bearer());
        out
    }

    pub fn restore(raw: &str) -> Option<Self> {
        let (code, bearer) = raw.split_once('\n')?;
        let review = CopyReview::from_code(code)?;
        let proof = CopyProof::presented(bearer)?;
        (proof.installation_id == review.plan.installation_id
            && proof.device_id == review.plan.device_id
            && proof.operation_id == review.plan.operation_id
            && proof.verifier() == review.verifier_bytes()?)
        .then_some(Self { review, proof })
    }

    pub async fn status(&self) -> Result<CopyState, &'static str> {
        self.request(&transport()?, COPY_STATUS_PATH, None).await
    }

    /// The caller already confirmed locally. Re-read the protected source after
    /// checking approval, but never read or resend it to reconcile a receipt.
    pub async fn commit(
        &self,
        read_source: impl AsyncFnOnce() -> Result<CopyInput, &'static str>,
    ) -> Result<CopyState, &'static str> {
        let client = transport()?;
        self.commit_after(
            self.request(&client, COPY_STATUS_PATH, None),
            read_source,
            async |body| self.request(&client, COPY_COMMIT_PATH, Some(body)).await,
        )
        .await
    }

    async fn commit_after(
        &self,
        approval: impl Future<Output = Result<CopyState, &'static str>>,
        read_source: impl AsyncFnOnce() -> Result<CopyInput, &'static str>,
        send: impl AsyncFnOnce(Vec<u8>) -> Result<CopyState, &'static str>,
    ) -> Result<CopyState, &'static str> {
        let state = approval.await?;
        if state != CopyState::Approved {
            return Ok(state);
        }
        let CopyInput {
            plan,
            provider,
            proxy,
        } = read_source().await?;
        let proxy = proxy.as_ref().map(|v| v.as_str());
        if plan != self.review.plan
            || !self.proof.matches_payload(
                &plan,
                &self.review.commitment_bytes().ok_or(INVALID)?,
                &provider,
                proxy,
            )
        {
            return Err("copy_source_changed");
        }
        let mut body = Vec::with_capacity(provider.len() + 1 + proxy.map_or(0, str::len));
        body.extend_from_slice(provider.as_bytes());
        body.push(b'\n');
        body.extend_from_slice(proxy.unwrap_or_default().as_bytes());
        let state = send(body).await?;
        if matches!(
            state,
            CopyState::Committed | CopyState::Expired | CopyState::Conflict
        ) {
            Ok(state)
        } else {
            Err(NETWORK)
        }
    }

    async fn request(
        &self,
        client: &Client,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<CopyState, &'static str> {
        let plan = &self.review.plan;
        let url = format!("{}{path}", plan.origin);
        let request = if let Some(body) = body {
            client
                .post(url)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(body)
        } else {
            client.get(url)
        };
        let mut r = request
            .bearer_auth(self.proof.bearer().as_str())
            .send()
            .await
            .map_err(|_| NETWORK)?;
        if r.status() == StatusCode::UNAUTHORIZED {
            return Err("copy_not_authorized");
        }
        if r.status() != StatusCode::OK
            || r.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(';').next())
                != Some("application/json")
        {
            return Err(NETWORK);
        }
        let mut data = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(|_| NETWORK)? {
            if chunk.len() > 2048 - data.len() {
                return Err(RESPONSE);
            }
            data.extend_from_slice(&chunk);
        }
        parse_reply(plan, &data)
    }
}

fn transport() -> Result<Client, &'static str> {
    Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|_| NETWORK)
}

fn parse_reply(plan: &CopyPlan, raw: &[u8]) -> Result<CopyState, &'static str> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        data: Reply,
        meta: super::request::Meta,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        installation_id: Uuid,
        device_id: Uuid,
        operation_id: Uuid,
        state: CopyState,
    }
    let r: Envelope = serde_json::from_slice(raw).map_err(|_| RESPONSE)?;
    if r.meta.api_version != "1"
        || r.meta.request_id.is_nil()
        || (
            r.data.installation_id,
            r.data.device_id,
            r.data.operation_id,
        ) != (plan.installation_id, plan.device_id, plan.operation_id)
    {
        return Err(RESPONSE);
    }
    Ok(r.data.state)
}

#[cfg(test)]
mod tests;
