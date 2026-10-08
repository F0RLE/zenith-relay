use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use url::Url;

use super::super::{is_http_endpoint, url_has_userinfo};
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
        if !is_http_endpoint(&base) || url_has_userinfo(&base) {
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
        let mut stats_request = self
            .client
            .get(self.endpoint(path, site)?)
            .header("Accept", "application/json");
        if authenticated {
            stats_request = stats_request.bearer_auth(&self.api_key);
        }
        let (stats_response, permit) = self
            .scope
            .send(&self.client, stats_request, HttpClass::Ordinary)
            .await
            .map_err(|_| S::Unavailable)?;
        self.hints.observe(stats_response.headers());
        match stats_response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(S::Unauthorized),
            429 => return Err(S::RateLimited),
            300..=399 | 404 | 405 => return Err(S::Unsupported),
            _ => return Err(S::Unavailable),
        }
        if stats_response
            .headers()
            .get("content-type")
            .and_then(|header_value| header_value.to_str().ok())
            .is_some_and(|header_value| header_value.to_ascii_lowercase().contains("text/html"))
        {
            return Err(S::Unsupported);
        }
        if stats_response
            .content_length()
            .is_some_and(|length| length > MAX_STATS_BYTES as u64)
        {
            return Err(S::InvalidResponse);
        }
        let mut response_bytes = Vec::new();
        let mut response_stream = stats_response.bytes_stream();
        while let Some(chunk) = response_stream.next().await {
            let chunk_bytes = chunk.map_err(|_| S::Unavailable)?;
            if response_bytes.len().saturating_add(chunk_bytes.len()) > MAX_STATS_BYTES {
                return Err(S::InvalidResponse);
            }
            response_bytes.extend_from_slice(&chunk_bytes);
        }
        drop(permit);
        serde_json::from_slice(&response_bytes).map_err(|_| S::InvalidResponse)
    }
}
