//! Observational analytics, isolated from quota admission, routing and billing.
use super::{
    db_error, params, sql_u64, Connection, ErrorCode, LocalPoolError, Result, TelemetryDb,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use zenith_relay_core::quota::QuotaWindow;

mod report;
#[cfg(test)]
mod tests;
mod workload;

pub(crate) use report::{WeeklyCapacityQuery, WeeklyCapacityReport};

pub(super) const HOUR_MS: u64 = 3_600_000;
pub(super) const DAY_MS: u64 = 24 * HOUR_MS;
const MIN_CONSUMED: u16 = 500;
const MAX_ANCHOR_AGE: u64 = 3 * DAY_MS;
const MAX_LEDGERS: usize = 512;
const MAX_HISTORY: usize = 50_000;

pub(crate) struct CapacityObservation<'a> {
    /// Already hashed provider account/workspace identity, not an email or token.
    pub ledger: &'a str,
    pub account_ids: &'a [String],
    pub plan: &'a str,
    pub window: &'a QuotaWindow,
    pub pricing_revision: &'a str,
    pub now_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct Anchor {
    account_ids: Vec<String>,
    plan: String,
    pricing_revision: String,
    members: String,
    window: QuotaWindow,
    from_ms: u64,
    remaining: u16,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CapacityEventKind {
    Reset,
    PlanChanged,
    PricingChanged,
    Discontinuity,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Measurement {
    ledger: String,
    account_ids: Vec<String>,
    at_ms: u64,
    from_ms: u64,
    reset_at_ms: u64,
    plan: String,
    pricing_revision: String,
    consumed: u16,
    micro_usd: Option<u64>,
    workload: Option<workload::Workload>,
    event: Option<CapacityEventKind>,
}

pub(super) fn fingerprint(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn json_error(_: serde_json::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "Invalid weekly capacity record")
}

fn stored_anchor(connection: &Connection, ledger: &str) -> Result<Option<String>> {
    connection
        .query_row(
            "SELECT payload_json FROM weekly_capacity_anchors WHERE account_id = ?1",
            [ledger],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)
}

impl TelemetryDb {
    pub(crate) fn weekly_capacity_revision(&self) -> u64 {
        self.capacity_revision
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(crate) fn observe_weekly_capacity(
        &self,
        observation: CapacityObservation<'_>,
        catalog: &zenith_relay_core::pricing::PricingCatalog,
        pricing: &zenith_relay_core::pricing::PricingContext,
    ) -> Result<()> {
        let CapacityObservation {
            ledger,
            account_ids,
            plan,
            window,
            pricing_revision,
            now_ms,
        } = observation;
        let plan = plan.trim().to_ascii_lowercase();
        let Some(remaining) = window
            .available_basis_points
            .filter(|value| *value <= 10_000)
        else {
            return Ok(());
        };
        let Some(reset_at_ms) = window.reset_at_ms.filter(|reset| *reset > now_ms) else {
            return Ok(());
        };
        if plan.is_empty()
            || plan == "unknown"
            || account_ids.is_empty()
            || account_ids.len() > MAX_LEDGERS
            || !window
                .window_minutes
                .is_some_and(|minutes| (6 * 1440..=8 * 1440).contains(&minutes))
            || !window
                .window_start_ms
                .is_some_and(|start| start <= window.observed_at_ms)
            || window.observed_at_ms > now_ms
            || now_ms - window.observed_at_ms > DAY_MS
        {
            return Ok(());
        }
        let revision = fingerprint(pricing_revision);
        let members = fingerprint(&account_ids.join("\0"));
        let identity =
            fingerprint(&serde_json::to_string(&(plan.as_str(), window)).map_err(json_error)?);
        let stamp = (
            window.observed_at_ms,
            identity,
            revision.clone(),
            members.clone(),
        );
        // Re-rendering or navigating never re-queries usage or rewrites observations.
        if self
            .capacity_seen
            .lock()
            .map_err(super::lock_error)?
            .get(ledger)
            == Some(&stamp)
        {
            return Ok(());
        }
        // Grouping several days of sends must not hold up request recording.
        // Pin anchor and usage to one WAL read snapshot, then fence the commit.
        let connection = self.lock_report_connection()?;
        let previous_json = stored_anchor(&connection, ledger)?;
        let previous = previous_json
            .as_deref()
            .map(serde_json::from_str::<Anchor>)
            .transpose()
            .map_err(json_error)?;
        let fresh = Anchor {
            account_ids: account_ids.to_vec(),
            plan: plan.clone(),
            pricing_revision: revision.clone(),
            members: members.clone(),
            window: window.clone(),
            from_ms: window.observed_at_ms,
            remaining,
        };
        let mut next = fresh.clone();
        let mut measurement = None;
        if let Some(previous) = previous {
            if window.observed_at_ms < previous.window.observed_at_ms {
                return Ok(());
            }
            if window.observed_at_ms == previous.window.observed_at_ms
                && previous.plan == plan
                && previous.pricing_revision == revision
                && previous.members == members
                && previous.window == *window
            {
                drop(connection);
                let connection = self.lock_connection()?;
                if stored_anchor(&connection, ledger)? == previous_json {
                    self.remember_capacity(ledger, stamp)?;
                }
                return Ok(());
            }
            let event = discontinuity(&previous, &fresh);
            if let Some(event) = event {
                measurement = Some(Measurement {
                    ledger: ledger.to_owned(),
                    account_ids: account_ids.to_vec(),
                    at_ms: window.observed_at_ms,
                    from_ms: window.observed_at_ms,
                    reset_at_ms,
                    plan: plan.clone(),
                    pricing_revision: revision.clone(),
                    consumed: 0,
                    micro_usd: None,
                    workload: None,
                    event: Some(event),
                });
            } else {
                let consumed = previous.remaining.saturating_sub(remaining);
                if consumed >= MIN_CONSUMED && window.observed_at_ms - previous.from_ms >= 60_000 {
                    // Adjacent intervals must not count the same second twice. The
                    // retained usage clock has second precision, not request-start precision.
                    let from_ms = (previous.from_ms / 1000 + 1) * 1000;
                    let priced = workload::measure(
                        &connection,
                        account_ids,
                        from_ms,
                        window.observed_at_ms,
                        catalog,
                        pricing,
                    )?;
                    if let Some((cost, workload)) = priced {
                        let capacity = u128::from(cost) * 10_000 / u128::from(consumed);
                        if let Ok(capacity) = u64::try_from(capacity) {
                            if capacity > 0 && capacity <= 9_007_199_254_740_991 {
                                measurement = Some(Measurement {
                                    ledger: ledger.to_owned(),
                                    account_ids: account_ids.to_vec(),
                                    at_ms: window.observed_at_ms,
                                    from_ms: previous.from_ms,
                                    reset_at_ms,
                                    plan: plan.clone(),
                                    pricing_revision: revision.clone(),
                                    consumed,
                                    micro_usd: Some(capacity),
                                    workload: Some(workload),
                                    event: None,
                                });
                            }
                        }
                    }
                    // An unpriced/incomplete block cannot contaminate the next one.
                } else {
                    next.from_ms = previous.from_ms;
                    next.remaining = previous.remaining;
                }
            }
        }
        drop(connection);
        let mut connection = self.lock_connection()?;
        // A concurrent history clear or newer observer owns the baseline now.
        // Never resurrect its removed observations from our older read snapshot.
        if stored_anchor(&connection, ledger)? != previous_json {
            return Ok(());
        }
        let transaction = connection.transaction().map_err(db_error)?;
        transaction.execute(
            "INSERT INTO weekly_capacity_anchors(account_id, observed_at_ms, payload_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id) DO UPDATE SET observed_at_ms = excluded.observed_at_ms, payload_json = excluded.payload_json",
            params![ledger, sql_u64(window.observed_at_ms), serde_json::to_string(&next).map_err(json_error)?],
        ).map_err(db_error)?;
        let changed = measurement.is_some();
        if let Some(measurement) = measurement {
            transaction.execute(
                "INSERT INTO weekly_capacity_history(account_id, observed_at_ms, payload_json) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, observed_at_ms) DO UPDATE SET payload_json = excluded.payload_json",
                params![ledger, sql_u64(window.observed_at_ms), serde_json::to_string(&measurement).map_err(json_error)?],
            ).map_err(db_error)?;
            transaction
                .execute(
                    "DELETE FROM weekly_capacity_history WHERE observed_at_ms < ?1",
                    [sql_u64(now_ms.saturating_sub(30 * DAY_MS))],
                )
                .map_err(db_error)?;
            transaction.execute(
                "DELETE FROM weekly_capacity_history WHERE (account_id, observed_at_ms) IN
                 (SELECT account_id, observed_at_ms FROM weekly_capacity_history ORDER BY observed_at_ms DESC LIMIT -1 OFFSET ?1)",
                [MAX_HISTORY as i64],
            ).map_err(db_error)?;
        }
        transaction.execute(
            "DELETE FROM weekly_capacity_anchors WHERE account_id IN
             (SELECT account_id FROM weekly_capacity_anchors ORDER BY observed_at_ms DESC LIMIT -1 OFFSET ?1)",
            [MAX_LEDGERS as i64],
        ).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        if changed {
            self.capacity_revision
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        self.remember_capacity(ledger, stamp)
    }

    fn remember_capacity(&self, ledger: &str, stamp: (u64, String, String, String)) -> Result<()> {
        let mut seen = self.capacity_seen.lock().map_err(super::lock_error)?;
        if seen.len() >= MAX_LEDGERS && !seen.contains_key(ledger) {
            seen.clear();
        }
        seen.insert(ledger.to_owned(), stamp);
        Ok(())
    }
}

fn discontinuity(previous: &Anchor, next: &Anchor) -> Option<CapacityEventKind> {
    if previous.plan != next.plan {
        return Some(CapacityEventKind::PlanChanged);
    }
    if previous.pricing_revision != next.pricing_revision {
        return Some(CapacityEventKind::PricingChanged);
    }
    if previous.window.provider_cycle_id.is_some()
        && next.window.provider_cycle_id.is_some()
        && previous.window.provider_cycle_id != next.window.provider_cycle_id
        || next
            .window
            .window_start_ms
            .is_some_and(|start| start >= previous.window.reset_at_ms.unwrap_or(u64::MAX))
    {
        return Some(CapacityEventKind::Reset);
    }
    if previous.window.kind != next.window.kind
        || previous.window.window_minutes != next.window.window_minutes
        || previous.window.provider_cycle_id != next.window.provider_cycle_id
        || previous
            .window
            .window_start_ms
            .unwrap_or(0)
            .abs_diff(next.window.window_start_ms.unwrap_or(0))
            > 120_000
        || previous
            .window
            .reset_at_ms
            .unwrap_or(0)
            .abs_diff(next.window.reset_at_ms.unwrap_or(0))
            > 120_000
        || next.remaining > previous.window.available_basis_points.unwrap_or(0)
    {
        return Some(CapacityEventKind::Discontinuity);
    }
    if previous.members != next.members
        || next.window.observed_at_ms == previous.window.observed_at_ms
        || next.window.observed_at_ms.saturating_sub(previous.from_ms) > MAX_ANCHOR_AGE
    {
        return Some(CapacityEventKind::Discontinuity);
    }
    None
}
