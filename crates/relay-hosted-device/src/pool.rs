use super::{DeviceKey, DeviceRead, RESPONSE, request::nullable};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PoolAction {
    Include,
    Exclude,
}

impl PoolAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PoolState {
    Enabled,
    Paused,
    Trashed,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoolMembership {
    pub state: PoolState,
    pub in_pool: bool,
    pub draining: bool,
    #[serde(deserialize_with = "nullable")]
    pub action: Option<PoolAction>,
    #[serde(deserialize_with = "nullable")]
    pub revision: Option<String>,
}

impl PoolMembership {
    pub(super) fn valid(&self, allowed: bool) -> bool {
        !(self.state == PoolState::Enabled && self.draining
            || self.state == PoolState::Trashed && self.in_pool)
            && match (self.action, self.revision.as_deref()) {
                (None, None) => true,
                (Some(a), Some(r)) if allowed && revision(r) => {
                    if a == PoolAction::Exclude {
                        self.in_pool
                    } else {
                        !self.in_pool && self.state == PoolState::Enabled
                    }
                }
                _ => false,
            }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoolChange {
    pub source_id: Uuid,
    pub action: PoolAction,
    pub expected_revision: String,
}

impl PoolChange {
    pub fn valid(&self) -> bool {
        !self.source_id.is_nil() && revision(&self.expected_revision)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoolReview {
    pub operation_id: Uuid,
    pub source_id: Uuid,
    pub action: PoolAction,
    pub expires_at_s: i64,
}

impl PoolReview {
    pub(super) fn valid(&self, grant_expires: i64) -> bool {
        !self.operation_id.is_nil()
            && !self.source_id.is_nil()
            && (1..=grant_expires).contains(&self.expires_at_s)
    }
}

fn revision(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

impl DeviceKey {
    pub async fn prepare_pool_change(
        &self,
        change: &PoolChange,
    ) -> Result<PoolReview, &'static str> {
        if !change.valid() {
            return Err("invalid_pool_review");
        }
        let body = serde_json::to_vec(change).map_err(|_| "invalid_pool_review")?;
        let page: DeviceRead<PoolReview> = self.send(self.read_url("pool")?, Some(body)).await?;
        if !self.expected_read(page.installation_id, page.device_id, page.expires_at_s)
            || !page.result.valid(page.expires_at_s)
            || (page.result.source_id, page.result.action) != (change.source_id, change.action)
        {
            return Err(RESPONSE);
        }
        Ok(page.result)
    }
}
