use super::{DeviceKey, RESPONSE};
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use std::{net::SocketAddr, time::Duration};
use url::Url;
use uuid::Uuid;

const NETWORK: &str = "device_server_unavailable";

// The property must be present even when it is null.
pub(super) fn nullable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
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
struct Envelope<T> {
    data: T,
    meta: Meta,
}

pub(super) fn model_id(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:/-".contains(&c))
}

impl DeviceKey {
    pub(super) fn read_url(&self, resource: &'static str) -> Result<Url, &'static str> {
        Url::parse(&format!(
            "{}/api/hosted/v1/device/{resource}",
            self.target.origin
        ))
        .map_err(|_| NETWORK)
    }

    pub(super) async fn read<T: DeserializeOwned>(&self, url: Url) -> Result<T, &'static str> {
        self.send(url, None).await
    }

    pub(super) async fn send<T: DeserializeOwned>(
        &self,
        url: Url,
        body: Option<Vec<u8>>,
    ) -> Result<T, &'static str> {
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
        let client = client.build().map_err(|_| NETWORK)?;
        let posting = body.is_some();
        let req = match body {
            Some(body) => client
                .post(url)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body),
            None => client.get(url),
        };
        let mut r = req
            .bearer_auth(self.bearer().as_str())
            .send()
            .await
            .map_err(|_| NETWORK)?;
        match r.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED => return Err("device_not_authorized"),
            StatusCode::BAD_REQUEST if posting => return Err("invalid_pool_review"),
            StatusCode::BAD_REQUEST => return Err("invalid_device_filter"),
            StatusCode::CONFLICT if posting => return Err("desktop_pool_changed"),
            StatusCode::NOT_FOUND => return Err("device_endpoint_unavailable"),
            _ => return Err(NETWORK),
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
        let reply: Envelope<T> = serde_json::from_slice(&body).map_err(|_| RESPONSE)?;
        if reply.meta.api_version != "1" || reply.meta.request_id.is_nil() {
            return Err(RESPONSE);
        }
        Ok(reply.data)
    }

    pub(super) fn expected_read(
        &self,
        installation: Uuid,
        device: Uuid,
        expires_at_s: i64,
    ) -> bool {
        installation == self.target.installation
            && device == self.target.device
            && (1..=253402300799).contains(&expires_at_s)
    }
}
