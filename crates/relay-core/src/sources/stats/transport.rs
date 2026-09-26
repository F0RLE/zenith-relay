use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use url::Url;

use super::SourceStatsStatus;
use crate::scheduler::refresh::http::{HttpClass, ManagementHttpScope};

pub(super) type StatsResult<T> = Result<T, SourceStatsStatus>;
const MAX_STATS_BYTES: usize = 1024 * 1024;

pub(super) struct StatsClient {
    client: reqwest::Client,
    base: Url,
    site: Url,
    api_key: String,
    pub(super) hints: crate::sources::observations::SourceReadHints,
    scope: ManagementHttpScope,
}

impl StatsClient {
    #[cfg(test)]
    pub(super) fn new(base_url: &str, api_key: &str) -> Result<Self, String> {
        Self::new_with_scope(base_url, api_key, ManagementHttpScope::default())
    }

    pub(super) fn new_with_scope(
        base_url: &str,
        api_key: &str,
        scope: ManagementHttpScope,
    ) -> Result<Self, String> {
        let invalid = || "source stats base URL is invalid".to_owned();
        let mut base = super::super::normalized_base_url(base_url).map_err(|_| invalid())?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(invalid());
        }
        base.set_query(None);
        base.set_fragment(None);
        let prefix = base.path().trim_end_matches('/').to_owned();
        let site_prefix = prefix.strip_suffix("/v1").unwrap_or(&prefix);
        let mut site = base.clone();
        site.set_path(&format!("{site_prefix}/"));
        base.set_path(&format!("{prefix}/"));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(6))
            .build()
            .map_err(|_| "source stats request could not be initialized".to_owned())?;
        Ok(Self {
            client,
            base,
            site,
            api_key: api_key.to_owned(),
            hints: Default::default(),
            scope,
        })
    }

    pub(super) fn host(&self) -> Option<&str> {
        self.base.host_str()
    }

    pub(super) fn endpoint(&self, path: &str, site: bool) -> StatsResult<Url> {
        let endpoint = if site { &self.site } else { &self.base }
            .join(path)
            .map_err(|_| SourceStatsStatus::InvalidResponse)?;
        if endpoint.origin() != self.base.origin() {
            return Err(SourceStatsStatus::InvalidResponse);
        }
        Ok(endpoint)
    }

    pub(super) async fn get(
        &self,
        path: &str,
        site: bool,
        authenticated: bool,
    ) -> StatsResult<Value> {
        use SourceStatsStatus as S;
        let mut request = self
            .client
            .get(self.endpoint(path, site)?)
            .header("Accept", "application/json");
        if authenticated {
            request = request.bearer_auth(&self.api_key);
        }
        let (response, permit) = self
            .scope
            .send(&self.client, request, HttpClass::Ordinary)
            .await
            .map_err(|_| S::Unavailable)?;
        self.hints.observe(response.headers());
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(S::Unauthorized),
            429 => return Err(S::RateLimited),
            300..=399 | 404 | 405 => return Err(S::Unsupported),
            _ => return Err(S::Unavailable),
        }
        if response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("text/html"))
        {
            return Err(S::Unsupported);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_STATS_BYTES as u64)
        {
            return Err(S::InvalidResponse);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| S::Unavailable)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_STATS_BYTES {
                return Err(S::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        drop(permit);
        serde_json::from_slice(&bytes).map_err(|_| S::InvalidResponse)
    }
}
