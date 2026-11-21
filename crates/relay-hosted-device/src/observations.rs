use super::{DeviceKey, RESPONSE, request::nullable};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

const FILTER: &str = "invalid_device_filter";
const MAX_MS: i64 = 253402300799999;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadScope {
    #[serde(rename = "overview.read")]
    Overview,
    #[serde(rename = "usage.read")]
    Usage,
}

impl ReadScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview.read",
            Self::Usage => "usage.read",
        }
    }
}

pub fn valid_read_scopes(scopes: &[ReadScope]) -> bool {
    scopes.len() <= 2 && scopes.windows(2).all(|w| w[0] < w[1])
}

pub fn valid_hours(hours: i32) -> bool {
    matches!(hours, 24 | 168 | 720)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    Reserved,
    Dispatched,
    Completed,
    Incomplete,
    Failed,
    Uncertain,
}

impl Execution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Dispatched => "dispatched",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageFilter {
    pub hours: i32,
    pub model: Option<String>,
    pub execution: Option<Execution>,
    pub as_of_ms: Option<i64>,
    pub before: Option<Uuid>,
}

impl UsageFilter {
    pub fn valid(&self) -> bool {
        valid_hours(self.hours)
            && self.model.as_deref().is_none_or(super::request::model_id)
            && self.as_of_ms.is_none_or(|v| (1..=MAX_MS).contains(&v))
            && self
                .before
                .is_none_or(|id| !id.is_nil() && self.as_of_ms.is_some())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceRead<T> {
    pub installation_id: Uuid,
    pub device_id: Uuid,
    pub expires_at_s: i64,
    pub result: T,
}

pub type DeviceAccess = DeviceRead<Vec<ReadScope>>;
pub type DeviceOverview = DeviceRead<Overview>;
pub type DeviceUsage = DeviceRead<UsagePage>;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Overview {
    pub hours: i32,
    pub as_of_ms: i64,
    pub requests: String,
    pub token_samples: String,
    pub partial_tokens: String,
    #[serde(deserialize_with = "nullable")]
    pub total_tokens: Option<String>,
    pub errors: String,
    pub active_requests: String,
    pub active_attempts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<OverviewMetrics>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OverviewMetrics {
    #[serde(deserialize_with = "nullable")]
    pub pool: Option<PoolCapacity>,
    #[serde(deserialize_with = "nullable")]
    pub generation: Option<GenerationRate>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoolCapacity {
    pub members: String,
    pub ready: String,
    pub busy: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRate {
    pub samples: String,
    pub tokens_per_second: f64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsagePage {
    pub hours: i32,
    pub as_of_ms: i64,
    #[serde(deserialize_with = "nullable")]
    pub model: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub execution: Option<Execution>,
    pub items: Vec<UsageRow>,
    #[serde(deserialize_with = "nullable")]
    pub next_before: Option<Uuid>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRow {
    pub request_id: Uuid,
    pub started_at_ms: i64,
    pub model: String,
    pub execution: Execution,
    #[serde(deserialize_with = "nullable")]
    pub tokens: Option<String>,
    pub partial_tokens: bool,
    #[serde(deserialize_with = "nullable")]
    pub diagnostic_code: Option<String>,
}

fn count(s: &str) -> Option<u128> {
    let n: u128 = s.parse().ok()?;
    (n.to_string() == s).then_some(n)
}

impl Overview {
    fn valid(&self, hours: i32) -> Option<()> {
        let requests = count(&self.requests)?;
        let samples = count(&self.token_samples)?;
        (self.hours == hours
            && (1..=MAX_MS).contains(&self.as_of_ms)
            && samples <= requests
            && count(&self.partial_tokens)? <= requests
            && count(&self.errors)? <= requests
            && count(&self.active_attempts)? <= count(&self.active_requests)?
            && self.metrics.as_ref().is_none_or(OverviewMetrics::valid)
            && match self.total_tokens.as_deref() {
                Some("0") if requests == 0 => true,
                Some(v) => samples > 0 && count(v).is_some(),
                None => requests > 0 && samples == 0,
            })
        .then_some(())
    }
}

impl OverviewMetrics {
    fn valid(&self) -> bool {
        self.pool.as_ref().is_none_or(|p| {
            let ready_and_busy = count(&p.ready)
                .zip(count(&p.busy))
                .and_then(|(r, b)| r.checked_add(b));
            ready_and_busy
                .zip(count(&p.members))
                .is_some_and(|(active, total)| active <= total)
        }) && self.generation.as_ref().is_none_or(|g| {
            count(&g.samples).is_some_and(|n| n > 0)
                && g.tokens_per_second > 0.0
                && g.tokens_per_second <= 1000.0
        })
    }
}

impl UsagePage {
    fn valid(&self, f: &UsageFilter) -> bool {
        if self.hours != f.hours
            || self.model != f.model
            || self.execution != f.execution
            || !(1..=MAX_MS).contains(&self.as_of_ms)
            || f.as_of_ms.is_some_and(|t| self.as_of_ms != t)
            || self.items.len() > 32
            || self.next_before.is_some_and(|id| {
                self.items.len() != 32 || self.items.last().is_none_or(|r| r.request_id != id)
            })
        {
            return false;
        }
        let start = self.as_of_ms - i64::from(self.hours) * 3_600_000;
        let mut upper = self.as_of_ms - 1;
        let mut seen = HashSet::new();
        self.items.iter().all(|r| {
            let in_order = (start..=upper).contains(&r.started_at_ms);
            upper = r.started_at_ms;
            in_order
                && !r.request_id.is_nil()
                && Some(r.request_id) != f.before
                && seen.insert(r.request_id)
                && super::request::model_id(&r.model)
                && f.model.as_ref().is_none_or(|m| &r.model == m)
                && f.execution.is_none_or(|e| r.execution == e)
                && r.tokens.as_deref().is_none_or(|n| count(n).is_some())
                && r.diagnostic_code.as_deref().is_none_or(|code| {
                    code.len() <= 64
                        && code.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                        && code
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                })
        })
    }
}

impl DeviceKey {
    async fn observation<T: serde::de::DeserializeOwned>(
        &self,
        url: url::Url,
    ) -> Result<DeviceRead<T>, &'static str> {
        let v: DeviceRead<T> = self.read(url).await?;
        if !self.expected_read(v.installation_id, v.device_id, v.expires_at_s) {
            return Err(RESPONSE);
        }
        Ok(v)
    }

    pub async fn access(&self) -> Result<DeviceAccess, &'static str> {
        let page = self
            .observation::<Vec<ReadScope>>(self.read_url("access")?)
            .await?;
        if !valid_read_scopes(&page.result) {
            return Err(RESPONSE);
        }
        Ok(page)
    }

    pub async fn overview(&self, hours: i32) -> Result<DeviceOverview, &'static str> {
        if !valid_hours(hours) {
            return Err(FILTER);
        }
        let requested = |resource| -> Result<url::Url, &'static str> {
            let mut url = self.read_url(resource)?;
            url.query_pairs_mut()
                .append_pair("hours", &hours.to_string());
            Ok(url)
        };
        let (page, with_metrics) = match self
            .observation::<Overview>(requested("overview/metrics")?)
            .await
        {
            Err("device_endpoint_unavailable") => (
                self.observation::<Overview>(requested("overview")?).await?,
                false,
            ),
            response => (response?, true),
        };
        if page.result.metrics.is_some() != with_metrics {
            return Err(RESPONSE);
        }
        page.result.valid(hours).ok_or(RESPONSE)?;
        Ok(page)
    }

    pub async fn usage(&self, filter: &UsageFilter) -> Result<DeviceUsage, &'static str> {
        if !filter.valid() {
            return Err(FILTER);
        }
        let mut url = self.read_url("usage")?;
        {
            let mut fields = url.query_pairs_mut();
            fields.append_pair("hours", &filter.hours.to_string());
            if let Some(m) = &filter.model {
                fields.append_pair("model", m);
            }
            if let Some(e) = filter.execution {
                fields.append_pair("execution", e.as_str());
            }
            if let Some(t) = filter.as_of_ms {
                fields.append_pair("as_of_ms", &t.to_string());
            }
            if let Some(id) = filter.before {
                fields.append_pair("before", &id.to_string());
            }
        }
        let page = self.observation::<UsagePage>(url).await?;
        if !page.result.valid(filter) {
            return Err(RESPONSE);
        }
        Ok(page)
    }
}
