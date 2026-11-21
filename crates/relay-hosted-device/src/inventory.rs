use super::request::{model_id, nullable};
use super::{DeviceKey, PoolMembership, PoolReview, RESPONSE, SCOPE};
use serde::{Deserialize, Serialize};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<PoolMembership>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_access: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_review: Option<PoolReview>,
}

impl DeviceKey {
    pub async fn connections(&self, after: Option<Uuid>) -> Result<DevicePage, &'static str> {
        self.connection_page(after, false).await
    }

    pub async fn connections_with_pool(
        &self,
        after: Option<Uuid>,
    ) -> Result<DevicePage, &'static str> {
        match self.connection_page(after, true).await {
            Err("device_endpoint_unavailable") => self.connections(after).await,
            result => result,
        }
    }

    async fn connection_page(
        &self,
        after: Option<Uuid>,
        pool: bool,
    ) -> Result<DevicePage, &'static str> {
        let mut url = self.read_url(if pool { "pool" } else { "connections" })?;
        if let Some(cursor) = after {
            url.query_pairs_mut()
                .append_pair("after", &cursor.to_string());
        }
        let p: DevicePage = self.read(url).await?;
        if p.scope != SCOPE
            || !self.expected_read(p.installation_id, p.device_id, p.expires_at_s)
            || p.pool_access.is_some() != pool
            || p.pool_review
                .as_ref()
                .is_some_and(|r| p.pool_access != Some(true) || !r.valid(p.expires_at_s))
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
                || !row.verified_models.iter().all(|s| model_id(s))
                || row.verified_models.windows(2).any(|w| w[0] >= w[1])
                || row.pool.is_some() != (pool && row.credential_state == "stored")
                || row
                    .pool
                    .as_ref()
                    .is_some_and(|s| !s.valid(p.pool_access == Some(true)))
            {
                return Err(RESPONSE);
            }
            prev = row.id;
        }
        Ok(p)
    }
}
