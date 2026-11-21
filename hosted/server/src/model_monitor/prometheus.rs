use super::{CAPACITY, SLOT_SECONDS};
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::value::RawValue;
use std::{net::SocketAddr, time::Duration};
use uuid::Uuid;

const METRIC: &str = "relay_hosted_model_probe_success";
const PROBE_MS: i64 = 300_000;
const PROBES: usize = 6;
const MAX_BYTES: usize = 128 * 1024;

pub(super) struct Transport {
    client: Client,
    endpoint: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Condition {
    Operational,
    Degraded,
    Unavailable,
}

impl Condition {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Operational => "operational",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }
}

impl Transport {
    pub(super) fn new(address: SocketAddr) -> Result<Self, &'static str> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err("model monitor requires a numeric loopback address and port");
        }
        let client = Client::builder()
            .no_proxy()
            .http1_only()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(4))
            .build()
            .map_err(|_| "model monitor transport unavailable")?;
        Ok(Self {
            client,
            endpoint: format!("http://{address}/api/v1/query"),
        })
    }

    pub(super) async fn read_slot(
        &self,
        slot: i64,
        ids: &[Uuid],
    ) -> Result<Vec<Option<Condition>>, &'static str> {
        let ids: Vec<_> = ids.iter().map(Uuid::to_string).collect();
        let query = format!(r#"{METRIC}{{publication_id=~"{}"}}[30m]"#, ids.join("|"));
        // Raw PromQL range selectors use (from, to]; shift by one millisecond
        // to select exactly the stored samples in [slot, slot + 30m).
        let time = format!("{}.999", slot + SLOT_SECONDS - 1);
        let limit = (CAPACITY + 1).to_string();
        const FAILED: &str = "model monitor query failed";
        const TOO_LARGE: &str = "model monitor response exceeds 128 KiB";
        let mut response = self
            .client
            .post(&self.endpoint)
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("query", query.as_str()),
                ("time", time.as_str()),
                ("timeout", "2s"),
                ("limit", limit.as_str()),
            ])
            .send()
            .await
            .map_err(|_| FAILED)?;
        if response.status() != StatusCode::OK {
            return Err(FAILED);
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(TOO_LARGE);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| FAILED)? {
            if chunk.len() > MAX_BYTES - body.len() {
                return Err(TOO_LARGE);
            }
            body.extend_from_slice(&chunk);
        }
        decode_slot(&body, slot, &ids)
    }
}

#[derive(Deserialize)]
struct Envelope<'a> {
    status: &'a str,
    #[serde(borrow)]
    data: Matrix<'a>,
    #[serde(default)]
    warnings: Vec<IgnoredAny>,
    #[serde(default)]
    infos: Vec<IgnoredAny>,
}

#[derive(Deserialize)]
struct Matrix<'a> {
    #[serde(rename = "resultType")]
    result_type: &'a str,
    #[serde(borrow)]
    result: Vec<Series<'a>>,
}

#[derive(Deserialize)]
struct Series<'a> {
    metric: Labels<'a>,
    #[serde(default, borrow)]
    values: Vec<(&'a RawValue, &'a str)>,
    #[serde(default)]
    histograms: Vec<IgnoredAny>,
}

#[derive(Deserialize)]
struct Labels<'a> {
    #[serde(rename = "__name__")]
    name: &'a str,
    publication_id: &'a str,
}

fn decode_slot(
    body: &[u8],
    slot: i64,
    ids: &[String],
) -> Result<Vec<Option<Condition>>, &'static str> {
    const INVALID: &str = "invalid or incomplete model monitor response";
    let envelope: Envelope<'_> = serde_json::from_slice(body).map_err(|_| INVALID)?;
    if envelope.status != "success"
        || envelope.data.result_type != "matrix"
        || !envelope.warnings.is_empty()
        || !envelope.infos.is_empty()
        || envelope.data.result.len() > CAPACITY
    {
        return Err(INVALID);
    }
    let mut states = vec![None; ids.len()];
    let mut seen = vec![false; ids.len()];
    for series in envelope.data.result {
        let index = ids
            .iter()
            .position(|id| id == series.metric.publication_id)
            .filter(|_| series.metric.name == METRIC)
            .ok_or(INVALID)?;
        states[index] = if !seen[index] && series.histograms.is_empty() {
            classify(slot * 1000, &series.values)
        } else {
            None
        };
        seen[index] = true;
    }
    Ok(states)
}

fn classify(start_ms: i64, samples: &[(&RawValue, &str)]) -> Option<Condition> {
    if samples.len() != PROBES {
        return None;
    }
    let mut successes = 0;
    // Exactly one chronologically ordered outcome per 5-minute bin.
    for (i, &(timestamp, value)) in samples.iter().enumerate() {
        let timestamp = millis(timestamp.get())?;
        let from = start_ms + i as i64 * PROBE_MS;
        if !(from..from + PROBE_MS).contains(&timestamp) {
            return None;
        }
        match value {
            "0" => (),
            "1" => successes += 1,
            _ => return None,
        }
    }
    Some(match successes {
        0 => Condition::Unavailable,
        PROBES => Condition::Operational,
        _ => Condition::Degraded,
    })
}

fn millis(raw: &str) -> Option<i64> {
    let (seconds, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if !seconds.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 3
    {
        return None;
    }
    let whole = seconds.parse::<i64>().ok()?.checked_mul(1000)?;
    let fraction_ms = fraction
        .bytes()
        .zip([100, 10, 1])
        .map(|(digit, place)| i64::from(digit - b'0') * place)
        .sum();
    whole.checked_add(fraction_ms)
}

#[cfg(test)]
mod tests;
