use serde_json::Value;

use super::{
    transport::StatsResult, SourceBalanceKind as Kind, SourceProviderStats as Stats,
    SourceStatsCurrency as Currency, SourceStatsProvider as Provider, SourceStatsStatus as Status,
};

pub(in crate::sources) fn zenith_stats(payload: &Value) -> StatsResult<Stats> {
    let data = data(payload);
    let money = |names: &[&str], cents| {
        names
            .iter()
            .find_map(|name| amount(data.get(name), 1))
            .or_else(|| amount(data.get(cents), 10_000))
    };
    let balance = money(
        &["displayBalanceMicrousd", "balanceMicrousd"],
        "balanceCents",
    );
    let spent = money(&["displaySpentMicrousd", "spentMicrousd"], "spentCents");
    if balance.is_none() || spent.is_none() {
        return Err(Status::InvalidResponse);
    }
    let mut stats = Stats::empty(Provider::Zenith, Status::Available);
    stats.amount(Currency::Usd, balance, spent);
    stats.requests = counter(data.get("requests"));
    stats.total_tokens = counter(data.get("totalTokens").or_else(|| data.get("total_tokens")));
    Ok(stats)
}

pub(in crate::sources) fn openrouter_stats(payload: &Value) -> StatsResult<Stats> {
    let data = data(payload);
    let credits = amount(
        data.get("total_credits")
            .or_else(|| data.get("totalCredits")),
        1_000_000,
    )
    .ok_or(Status::InvalidResponse)?;
    let spent = amount(
        data.get("total_usage").or_else(|| data.get("totalUsage")),
        1_000_000,
    )
    .ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::OpenRouter, Status::Available);
    stats.amount(
        Currency::Usd,
        Some(credits.checked_sub(spent).ok_or(Status::InvalidResponse)?),
        Some(spent),
    );
    Ok(stats)
}

pub(super) fn openrouter_key_stats(payload: &Value) -> StatsResult<Stats> {
    let data = data(payload);
    if data.get("limit").is_none() || data.get("usage").is_none() {
        return Err(Status::InvalidResponse);
    }
    let mut stats = Stats::empty(Provider::OpenRouter, Status::Available);
    stats.balance_kind = Kind::KeyQuota;
    stats.balance_unlimited = data.get("limit").is_some_and(Value::is_null);
    // Lifetime usage cannot reconstruct a periodically reset key allowance.
    stats.amount(
        Currency::Usd,
        if stats.balance_unlimited {
            None
        } else {
            amount(data.get("limit_remaining"), 1_000_000)
        },
        amount(data.get("usage"), 1_000_000),
    );
    complete(stats)
}

pub(super) fn sub2api_stats(payload: &Value) -> StatsResult<Stats> {
    let data = data(payload);
    let mode = data.get("mode").and_then(Value::as_str);
    if !matches!(mode, Some("unrestricted" | "quota_limited"))
        && !(data.get("isValid").is_some_and(Value::is_boolean)
            && data.get("remaining").is_some()
            && data.get("unit").is_some())
    {
        return Err(Status::Unsupported);
    }
    let mut stats = Stats::empty(Provider::Sub2Api, Status::Available);
    let quota = data.get("quota").filter(|value| value.is_object());
    stats.balance_kind = if quota.is_some() || mode == Some("quota_limited") {
        Kind::KeyQuota
    } else if data.get("subscription").is_some_and(Value::is_object) {
        Kind::Subscription
    } else {
        Kind::Wallet
    };
    let unit = quota
        .and_then(|value| value.get("unit"))
        .or_else(|| data.get("unit"))
        .and_then(Value::as_str);
    if unit.is_some_and(|unit| unit != "USD") {
        return Err(Status::InvalidResponse);
    }
    let balance = if let Some(quota) = quota {
        amount(quota.get("remaining"), 1_000_000)
    } else {
        amount(
            data.get("balance").or_else(|| data.get("remaining")),
            1_000_000,
        )
        .or_else(|| {
            data.get("rate_limits")?
                .as_array()?
                .iter()
                .filter_map(|window| amount(window.get("remaining"), 1_000_000))
                .min()
        })
    };
    stats.balance_unlimited =
        stats.balance_kind == Kind::Subscription && balance == Some(-1_000_000);
    let usage = data.pointer("/usage/total");
    // `cost` is list-price equivalent; only `actual_cost` proves provider spend.
    let spent = usage.and_then(|value| amount(value.get("actual_cost"), 1_000_000));
    stats.amount(
        Currency::Usd,
        if stats.balance_unlimited {
            None
        } else {
            balance
        },
        spent,
    );
    stats.requests = usage.and_then(|value| counter(value.get("requests")));
    stats.total_tokens = usage.and_then(|value| counter(value.get("total_tokens")));
    complete(stats)
}

pub(super) fn is_new_api(payload: &Value) -> bool {
    payload.pointer("/data/object").and_then(Value::as_str) == Some("token_usage")
}

pub(super) fn new_api_stats(payload: &Value, metadata: Option<&Value>) -> StatsResult<Stats> {
    if !is_new_api(payload) || payload.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(Status::InvalidResponse);
    }
    let data = data(payload);
    let divisor = metadata
        .and_then(valid_metadata)
        .and_then(|meta| amount(meta.get("quota_per_unit"), 1_000_000))
        .filter(|value| *value > 0);
    let convert = |name| {
        let quota = amount(data.get(name), 1_000_000)?;
        match divisor {
            Some(divisor) => divide_rounded(i128::from(quota) * 1_000_000, i128::from(divisor)),
            None => Some(quota),
        }
    };
    let mut stats = Stats::empty(Provider::NewApi, Status::Available);
    stats.balance_kind = Kind::KeyQuota;
    stats.balance_unlimited = data.get("unlimited_quota").and_then(Value::as_bool) == Some(true);
    stats.amount(
        if divisor.is_some() {
            Currency::Usd
        } else {
            Currency::Credits
        },
        if stats.balance_unlimited {
            None
        } else {
            convert("total_available")
        },
        convert("total_used"),
    );
    complete(stats)
}

pub(super) fn is_billing(payload: &Value) -> bool {
    payload.get("object").and_then(Value::as_str) == Some("billing_subscription")
}

pub(super) fn billing_stats(
    subscription: &Value,
    usage: &Value,
    metadata: Option<&Value>,
) -> StatsResult<Stats> {
    if !is_billing(subscription) || usage.get("object").and_then(Value::as_str) != Some("list") {
        return Err(Status::InvalidResponse);
    }
    let metadata = metadata.and_then(valid_metadata);
    let currency = match metadata
        .and_then(|meta| meta.get("quota_display_type"))
        .and_then(Value::as_str)
    {
        Some("CNY") => Currency::Cny,
        Some("TOKENS") => Currency::Credits,
        Some("USD") => Currency::Usd,
        Some(_) => return Err(Status::InvalidResponse),
        None if metadata
            .and_then(|meta| meta.get("display_in_currency"))
            .and_then(Value::as_bool)
            == Some(false) =>
        {
            Currency::Credits
        }
        None => Currency::Usd,
    };
    let limit =
        amount(subscription.get("hard_limit_usd"), 1_000_000).ok_or(Status::InvalidResponse)?;
    let spent = amount(usage.get("total_usage"), 10_000).ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::Billing, Status::Available);
    if metadata
        .and_then(|meta| meta.get("display_token_stat_enabled"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        stats.balance_kind = Kind::KeyQuota;
    }
    stats.amount(
        currency,
        Some(limit.checked_sub(spent).ok_or(Status::InvalidResponse)?),
        Some(spent),
    );
    Ok(stats)
}

pub(super) fn deepseek_stats(payload: &Value) -> StatsResult<Stats> {
    let entries = payload
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::Deepseek, Status::Available);
    for entry in entries {
        let currency = match entry.get("currency").and_then(Value::as_str) {
            Some("USD") => Currency::Usd,
            Some("CNY") => Currency::Cny,
            _ => continue,
        };
        if stats
            .amounts
            .iter()
            .any(|amount| amount.currency == currency)
        {
            return Err(Status::InvalidResponse);
        }
        stats.amount(
            currency,
            amount(entry.get("total_balance"), 1_000_000),
            None,
        );
    }
    complete(stats)
}

pub(super) fn siliconflow_stats(payload: &Value, host: Option<&str>) -> StatsResult<Stats> {
    let data = data(payload);
    if payload.get("status").and_then(Value::as_bool) != Some(true)
        || payload.get("code").and_then(Value::as_i64) != Some(20_000)
    {
        return Err(Status::InvalidResponse);
    }
    let currency = match host.map(str::to_ascii_lowercase).as_deref() {
        Some("api.siliconflow.cn") => Currency::Cny,
        Some("api.siliconflow.com") => Currency::Usd,
        _ => return Err(Status::Unsupported),
    };
    let total = amount(data.get("totalBalance"), 1_000_000).ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::SiliconFlow, Status::Available);
    stats.amount(currency, Some(total), None);
    complete(stats)
}

fn data(payload: &Value) -> &Value {
    payload
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(payload)
}

fn valid_metadata(payload: &Value) -> Option<&Value> {
    (payload.get("success").and_then(Value::as_bool) == Some(true)).then(|| data(payload))
}

fn complete(stats: Stats) -> StatsResult<Stats> {
    if stats.amounts.is_empty()
        && !stats.balance_unlimited
        && stats.requests.is_none()
        && stats.total_tokens.is_none()
    {
        Err(Status::InvalidResponse)
    } else {
        Ok(stats)
    }
}

fn counter(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

pub(super) fn amount(value: Option<&Value>, scale: u128) -> Option<i64> {
    let value = value?;
    let text = value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_number().map(ToString::to_string))?;
    let text = text.trim();
    if text.len() > 128 {
        return None;
    }
    let (negative, unsigned) = text
        .strip_prefix('-')
        .map_or((false, text), |text| (true, text));
    let value = i128::from(crate::pricing::decimal_to_scaled_allow_zero(unsigned, scale).ok()?);
    i64::try_from(if negative { -value } else { value }).ok()
}

fn divide_rounded(numerator: i128, divisor: i128) -> Option<i64> {
    let magnitude = numerator.abs();
    let rounded = magnitude / divisor + i128::from(magnitude % divisor >= (divisor + 1) / 2);
    i64::try_from(if numerator < 0 { -rounded } else { rounded }).ok()
}
