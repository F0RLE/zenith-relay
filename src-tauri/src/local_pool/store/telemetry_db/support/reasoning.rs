//! Bounded observational comparison, not a benchmark or a routing recommendation.
use super::super::{db_error, UsageLog};
use crate::local_pool::error::Result;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const RECENT_REQUEST_LIMIT: usize = 1000;
const MIN_GROUP_SAMPLES: usize = 5;

pub(crate) struct Samples {
    rows: Vec<Sample>,
    unavailable: Option<&'static str>,
}

struct Sample {
    success: bool,
    attempt: i64,
    effort: Option<String>,
    ttft_ms: Option<u64>,
    input_tokens: Option<u64>,
    cached_tokens: Option<u64>,
}

pub(crate) fn read_samples(connection: &Connection, anchor: Option<&UsageLog>) -> Result<Samples> {
    let unavailable = |reason| Samples {
        rows: Vec::new(),
        unavailable: Some(reason),
    };
    let Some(anchor) = anchor else {
        return Ok(unavailable("final_request_missing"));
    };
    if anchor.candidate_id.as_deref().is_none_or(str::is_empty)
        || anchor.resolved_model.as_deref().is_none_or(str::is_empty)
    {
        return Ok(unavailable("route_identity_missing"));
    }
    let Some(transport) = anchor
        .routing
        .as_ref()
        .and_then(|routing| routing.transport.as_ref())
    else {
        return Ok(unavailable("transport_observation_missing"));
    };
    let mode = match transport.mode {
        zenith_relay_core::usage::TransportMode::Native => "native",
        zenith_relay_core::usage::TransportMode::ProtocolConversion => "protocol_conversion",
        zenith_relay_core::usage::TransportMode::BasisPoints => "basis_points",
    };
    // Bound the indexed ID window BEFORE filtering by route. A scarce route must
    // not scan the entire log. CROSS JOIN keeps the bounded IDs before row lookups.
    // No prompts, raw JSON or identities leave this query.
    let mut statement = connection.prepare(&format!(
        "WITH recent AS (SELECT id FROM request_logs WHERE id <= ?1 ORDER BY id DESC LIMIT {RECENT_REQUEST_LIMIT})
         SELECT r.success, r.attempt, r.effective_reasoning_effort, r.ttft_ms,
                r.input_tokens, r.cached_input_tokens
         FROM request_logs a CROSS JOIN recent
         CROSS JOIN request_logs r ON r.id = recent.id
         WHERE a.id = ?1 AND r.local_key_id IS a.local_key_id AND r.source_id IS a.source_id
           AND r.candidate_id IS a.candidate_id AND r.account_id IS a.account_id
           AND r.requested_model IS a.requested_model AND r.resolved_model IS a.resolved_model
           AND r.wire_api IS a.wire_api AND r.transport IS a.transport
           AND r.service_tier IS a.service_tier AND r.applied_service_tier IS a.applied_service_tier
           AND r.created_at >= datetime(a.created_at, '-7 days') AND r.created_at <= a.created_at
           AND json_extract(CASE WHEN length(r.routing_json) <= 65536 AND json_valid(r.routing_json)
               THEN r.routing_json ELSE '{{}}' END, '$.transport.upstream') = ?2
           AND json_extract(CASE WHEN length(r.routing_json) <= 65536 AND json_valid(r.routing_json)
               THEN r.routing_json ELSE '{{}}' END, '$.transport.mode') = ?3"
    )).map_err(db_error)?;
    let rows = statement
        .query_map(
            params![anchor.id, transport.upstream.as_str(), mode],
            |row| {
                let number = |index| -> rusqlite::Result<Option<u64>> {
                    Ok(row
                        .get::<_, Option<i64>>(index)?
                        .and_then(|value| u64::try_from(value).ok()))
                };
                Ok(Sample {
                    success: row.get(0)?,
                    attempt: row.get(1)?,
                    effort: row.get(2)?,
                    ttft_ms: number(3)?,
                    input_tokens: number(4)?,
                    cached_tokens: number(5)?,
                })
            },
        )
        .map_err(db_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(Samples {
        rows,
        unavailable: None,
    })
}

#[derive(Default)]
struct Group {
    ttft: Vec<u64>,
    input: Vec<u64>,
    cache_rates: Vec<u64>,
}

fn percentile(values: &mut [u64], percent: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[(values.len() * percent).div_ceil(100).saturating_sub(1)])
}

pub(crate) fn document(samples: Samples) -> Value {
    let matched = samples.rows.len();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let (mut failed, mut multi_send, mut unknown_effort, mut missing_ttft) = (0, 0, 0, 0);
    for sample in samples.rows {
        if !sample.success {
            failed += 1;
            continue;
        }
        if sample.attempt != 1 {
            multi_send += 1;
            continue;
        }
        let Some(effort) = sample
            .effort
            .as_deref()
            .and_then(zenith_relay_core::normalize_reasoning_effort)
        else {
            unknown_effort += 1;
            continue;
        };
        let Some(ttft) = sample.ttft_ms else {
            missing_ttft += 1;
            continue;
        };
        let group = groups.entry(effort).or_default();
        group.ttft.push(ttft);
        if let Some(input) = sample.input_tokens {
            group.input.push(input);
        }
        if let (Some(input), Some(cached)) = (sample.input_tokens, sample.cached_tokens) {
            if input > 0 && cached <= input {
                group
                    .cache_rates
                    .push(((cached as u128 * 10000) / input as u128) as u64);
            }
        }
    }
    let enough_groups = groups
        .values()
        .filter(|group| group.ttft.len() >= MIN_GROUP_SAMPLES)
        .count();
    let status = if samples.unavailable.is_some() {
        "unavailable"
    } else if enough_groups >= 2 {
        "multiple_levels_observed"
    } else {
        "insufficient_samples"
    };
    let groups: Vec<_> = groups.into_iter().map(|(effort, mut group)| {
        let count = group.ttft.len();
        json!({
            "effectiveReasoningEffort": effort, "samples": count,
            "ttftP50Ms": percentile(&mut group.ttft, 50),
            "ttftP95Ms": if count >= 20 { percentile(&mut group.ttft, 95) } else { None },
            "inputTokenSamples": group.input.len(), "inputTokensP50": percentile(&mut group.input, 50),
            "cacheRateSamples": group.cache_rates.len(),
            "cachedInputFractionP50BasisPoints": percentile(&mut group.cache_rates, 50),
        })
    }).collect();
    json!({
        "status": status, "unavailableReason": samples.unavailable,
        "kind": "observational_not_controlled", "maxRecentRequests": RECENT_REQUEST_LIMIT,
        "lookbackDays": 7, "window": "at_or_before_selected_request",
        "minimumSamplesPerLevel": MIN_GROUP_SAMPLES, "minimumSamplesForP95": 20,
        "matchedRouteRequests": matched, "groups": groups,
        "excluded": { "unsuccessful": failed, "multipleSends": multi_send,
            "unknownEffectiveReasoning": unknown_effort, "missingTtft": missing_ttft },
        "limitations": "Same recorded key, source, candidate, account, requested/resolved model, client protocol/transport, requested/observed tier and upstream transport/mode. Successful single-send requests only; retries and multi-round discovery are excluded. Different prompts, context sizes, cache state, output types and provider load remain confounders. Percentiles use nearest rank. Missing levels are not evidence of unsupported capabilities. This is not a causal speed comparison or a recommendation to lower reasoning."
    })
}
