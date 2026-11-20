use super::{DeviceKey, RESPONSE, SCOPE};
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Deserializer, Serialize};
use std::{net::SocketAddr, time::Duration};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSource {
    pub id: Uuid,
    #[serde(deserialize_with = "nullable")]
    pub label: Option<String>,
    pub credential_state: String,
    pub proxy_bound: bool,
    pub verified_models: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DevicePage {
    pub installation_id: Uuid,
    pub device_id: Uuid,
    pub scope: String,
    pub expires_at_s: i64,
    pub items: Vec<SavedSource>,
    #[serde(deserialize_with = "nullable")]
    pub next_after: Option<Uuid>,
}

// Both fields must be present on the wire; missing pagination is not proof of the last page.
fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Meta {
    pub(super) api_version: String,
    pub(super) request_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    data: DevicePage,
    meta: Meta,
}

impl DeviceKey {
    pub async fn connections(&self, after: Option<Uuid>) -> Result<DevicePage, &'static str> {
        const NETWORK: &str = "device_server_unavailable";
        let target = self.target();
        let mut url = url::Url::parse(&format!(
            "{}/api/hosted/v1/device/connections",
            target.origin
        ))
        .map_err(|_| NETWORK)?;
        if let Some(cursor) = after {
            url.query_pairs_mut()
                .append_pair("after", &cursor.to_string());
        }
        let mut client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(10));
        if url.domain() == Some("localhost") {
            client = client.resolve(
                "localhost",
                SocketAddr::from(([127, 0, 0, 1], url.port_or_known_default().ok_or(NETWORK)?)),
            );
        }
        let mut r = client
            .build()
            .map_err(|_| NETWORK)?
            .get(url)
            .bearer_auth(self.bearer().as_str())
            .send()
            .await
            .map_err(|_| NETWORK)?;
        if r.status() == StatusCode::UNAUTHORIZED {
            return Err("device_not_authorized");
        }
        if r.status() != StatusCode::OK {
            return Err(NETWORK);
        }
        const MAX: usize = 768 * 1024;
        if r.content_length().is_some_and(|n| n > MAX as u64)
            || r.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(';').next())
                != Some("application/json")
        {
            return Err(RESPONSE);
        }
        let mut body = Vec::new();
        while let Some(chunk) = r.chunk().await.map_err(|_| NETWORK)? {
            if chunk.len() > MAX - body.len() {
                return Err(RESPONSE);
            }
            body.extend_from_slice(&chunk);
        }
        let reply: Envelope = serde_json::from_slice(&body).map_err(|_| RESPONSE)?;
        let p = reply.data;
        if reply.meta.api_version != "1"
            || reply.meta.request_id.is_nil()
            || p.scope != SCOPE
            || p.installation_id != target.installation
            || p.device_id != target.device
            || !(1..=253402300799).contains(&p.expires_at_s)
            || p.items.len() > 32
            || p.next_after
                .is_some_and(|n| p.items.len() != 32 || p.items.last().is_none_or(|r| r.id != n))
        {
            return Err(RESPONSE);
        }
        let mut prev = after.unwrap_or(Uuid::nil());
        for row in &p.items {
            if row.id <= prev
                || row.label.as_deref().is_some_and(|s| {
                    s.trim() != s || s.is_empty() || s.len() > 96 || s.chars().any(char::is_control)
                })
                || !matches!(row.credential_state.as_str(), "stored" | "revoked")
                || row.verified_models.len() > 128
                || row.credential_state == "revoked" && !row.verified_models.is_empty()
                || !row.verified_models.iter().all(|s| {
                    s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                        && s.len() <= 128
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"._:/-".contains(&c))
                })
                || row.verified_models.windows(2).any(|w| w[0] >= w[1])
            {
                return Err(RESPONSE);
            }
            prev = row.id;
        }
        Ok(p)
    }
}
