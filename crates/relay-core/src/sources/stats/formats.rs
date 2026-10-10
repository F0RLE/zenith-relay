use serde_json::Value;

use super::{
    transport::StatsResult, SourceBalanceKind as Kind, SourceProviderStats as Stats,
    SourceStatsCurrency as Currency, SourceStatsProvider as Provider, SourceStatsStatus as Status,
};

pub(in crate::sources) fn zenith_stats(stats_payload: &Value) -> StatsResult<Stats> {
    let response_data = stats_data(stats_payload);
    let money = |field_names: &[&str], cents_field| {
        field_names
            .iter()
            .find_map(|field_name| amount(response_data.get(field_name), 1))
            .or_else(|| amount(response_data.get(cents_field), 10_000))
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
    stats.requests = counter(response_data.get("requests"));
    stats.total_tokens = counter(
        response_data
            .get("totalTokens")
            .or_else(|| response_data.get("total_tokens")),
    );
    Ok(stats)
}

pub(in crate::sources) fn openrouter_stats(stats_payload: &Value) -> StatsResult<Stats> {
    let response_data = stats_data(stats_payload);
    let credits = amount(
        response_data
            .get("total_credits")
            .or_else(|| response_data.get("totalCredits")),
        1_000_000,
    )
    .ok_or(Status::InvalidResponse)?;
    let spent = amount(
        response_data
            .get("total_usage")
            .or_else(|| response_data.get("totalUsage")),
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

pub(super) fn openrouter_key_stats(stats_payload: &Value) -> StatsResult<Stats> {
    let response_data = stats_data(stats_payload);
    if response_data.get("limit").is_none() || response_data.get("usage").is_none() {
        return Err(Status::InvalidResponse);
    }
    let mut stats = Stats::empty(Provider::OpenRouter, Status::Available);
    stats.balance_kind = Kind::KeyQuota;
    stats.balance_unlimited = response_data.get("limit").is_some_and(Value::is_null);
    // Lifetime usage cannot reconstruct a periodically reset key allowance.
    stats.amount(
        Currency::Usd,
        if stats.balance_unlimited {
            None
        } else {
            amount(response_data.get("limit_remaining"), 1_000_000)
        },
        amount(response_data.get("usage"), 1_000_000),
    );
    complete(stats)
}

pub(super) fn sub2api_stats(stats_payload: &Value) -> StatsResult<Stats> {
    let response_data = stats_data(stats_payload);
    let account_mode = response_data.get("mode").and_then(Value::as_str);
    if !matches!(account_mode, Some("unrestricted" | "quota_limited"))
        && !(response_data.get("isValid").is_some_and(Value::is_boolean)
            && response_data.get("remaining").is_some()
            && response_data.get("unit").is_some())
    {
        return Err(Status::Unsupported);
    }
    let mut stats = Stats::empty(Provider::Sub2Api, Status::Available);
    let quota_object = response_data
        .get("quota")
        .filter(|quota_value| quota_value.is_object());
    stats.balance_kind = if quota_object.is_some() || account_mode == Some("quota_limited") {
        Kind::KeyQuota
    } else if response_data
        .get("subscription")
        .is_some_and(Value::is_object)
    {
        Kind::Subscription
    } else {
        Kind::Wallet
    };
    let unit = quota_object
        .and_then(|quota_value| quota_value.get("unit"))
        .or_else(|| response_data.get("unit"))
        .and_then(Value::as_str);
    if unit.is_some_and(|unit| unit != "USD") {
        return Err(Status::InvalidResponse);
    }
    let balance = if let Some(quota_object) = quota_object {
        amount(quota_object.get("remaining"), 1_000_000)
    } else {
        amount(
            response_data
                .get("balance")
                .or_else(|| response_data.get("remaining")),
            1_000_000,
        )
        .or_else(|| {
            response_data
                .get("rate_limits")?
                .as_array()?
                .iter()
                .filter_map(|window| amount(window.get("remaining"), 1_000_000))
                .min()
        })
    };
    stats.balance_unlimited =
        stats.balance_kind == Kind::Subscription && balance == Some(-1_000_000);
    let usage_totals = response_data.pointer("/usage/total");
    // `cost` is list-price equivalent; only `actual_cost` proves provider spend.
    let spent = usage_totals.and_then(|usage| amount(usage.get("actual_cost"), 1_000_000));
    stats.amount(
        Currency::Usd,
        if stats.balance_unlimited {
            None
        } else {
            balance
        },
        spent,
    );
    stats.requests = usage_totals.and_then(|usage| counter(usage.get("requests")));
    stats.total_tokens = usage_totals.and_then(|usage| counter(usage.get("total_tokens")));
    complete(stats)
}

pub(super) fn is_new_api(stats_payload: &Value) -> bool {
    stats_payload
        .pointer("/data/object")
        .and_then(Value::as_str)
        == Some("token_usage")
}

pub(super) fn new_api_stats(
    stats_payload: &Value,
    metadata_payload: Option<&Value>,
) -> StatsResult<Stats> {
    // New API uses `code: true`; older compatible deployments use `success: true`.
    // A present failure flag must not be overridden by the other field.
    let success_flags = ["code", "success"]
        .into_iter()
        .filter_map(|field| stats_payload.get(field))
        .collect::<Vec<_>>();
    if !is_new_api(stats_payload)
        || success_flags.is_empty()
        || success_flags
            .iter()
            .any(|flag_value| flag_value.as_bool() != Some(true))
    {
        return Err(Status::InvalidResponse);
    }
    let response_data = stats_data(stats_payload);
    let divisor = metadata_payload
        .and_then(valid_metadata_payload)
        .and_then(|metadata_object| amount(metadata_object.get("quota_per_unit"), 1_000_000))
        .filter(|quota_divisor| *quota_divisor > 0);
    let convert = |quota_field| {
        let quota = amount(response_data.get(quota_field), 1_000_000)?;
        match divisor {
            Some(divisor) => divide_rounded(i128::from(quota) * 1_000_000, i128::from(divisor)),
            None => Some(quota),
        }
    };
    let mut stats = Stats::empty(Provider::NewApi, Status::Available);
    stats.balance_kind = Kind::KeyQuota;
    stats.balance_unlimited = response_data
        .get("unlimited_quota")
        .and_then(Value::as_bool)
        == Some(true);
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

pub(super) fn is_billing(stats_payload: &Value) -> bool {
    stats_payload.get("object").and_then(Value::as_str) == Some("billing_subscription")
}

pub(super) fn billing_stats(
    subscription: &Value,
    usage: &Value,
    metadata_payload: Option<&Value>,
) -> StatsResult<Stats> {
    if !is_billing(subscription) || usage.get("object").and_then(Value::as_str) != Some("list") {
        return Err(Status::InvalidResponse);
    }
    // Billing fields retain legacy `_usd` names even when the site returns
    // CNY or raw quota. Without its status we cannot label the numbers.
    let metadata = metadata_payload
        .and_then(valid_metadata_payload)
        .ok_or(Status::InvalidResponse)?;
    let currency = match metadata.get("quota_display_type").and_then(Value::as_str) {
        Some("CNY") => Currency::Cny,
        Some("TOKENS") => Currency::Credits,
        Some("USD") => Currency::Usd,
        Some(_) => return Err(Status::InvalidResponse),
        None if metadata.get("display_in_currency").and_then(Value::as_bool) == Some(false) => {
            Currency::Credits
        }
        None => Currency::Usd,
    };
    let limit =
        amount(subscription.get("hard_limit_usd"), 1_000_000).ok_or(Status::InvalidResponse)?;
    let spent = amount(usage.get("total_usage"), 10_000).ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::Billing, Status::Available);
    if metadata
        .get("display_token_stat_enabled")
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

pub(super) fn deepseek_stats(stats_payload: &Value) -> StatsResult<Stats> {
    let balance_entries = stats_payload
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::Deepseek, Status::Available);
    for balance_entry in balance_entries {
        let currency = match balance_entry.get("currency").and_then(Value::as_str) {
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
            amount(balance_entry.get("total_balance"), 1_000_000),
            None,
        );
    }
    complete(stats)
}

pub(super) fn moonshot_stats(stats_payload: &Value, currency: Currency) -> StatsResult<Stats> {
    let response_data = stats_payload
        .get("data")
        .filter(|value| value.is_object())
        .ok_or(Status::InvalidResponse)?;
    if stats_payload.get("code").and_then(Value::as_i64) != Some(0)
        || stats_payload.get("status").and_then(Value::as_bool) != Some(true)
    {
        return Err(Status::InvalidResponse);
    }
    let balance =
        amount(response_data.get("available_balance"), 1_000_000).ok_or(Status::InvalidResponse)?;
    let mut stats = Stats::empty(Provider::Moonshot, Status::Available);
    stats.amount(currency, Some(balance), None);
    Ok(stats)
}

fn stats_data(stats_payload: &Value) -> &Value {
    stats_payload
        .get("data")
        .filter(|response_data| response_data.is_object())
        .unwrap_or(stats_payload)
}

fn valid_metadata_payload(stats_payload: &Value) -> Option<&Value> {
    (stats_payload.get("success").and_then(Value::as_bool) == Some(true))
        .then(|| stats_data(stats_payload))
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

fn counter(counter_value: Option<&Value>) -> Option<u64> {
    let counter_value = counter_value?;
    counter_value
        .as_u64()
        .or_else(|| counter_value.as_str()?.parse().ok())
}

pub(super) fn amount(amount_value: Option<&Value>, scale: u128) -> Option<i64> {
    let amount_value = amount_value?;
    let amount_text = amount_value
        .as_str()
        .map(str::to_owned)
        .or_else(|| amount_value.as_number().map(ToString::to_string))?;
    let amount_text = amount_text.trim();
    if amount_text.len() > 128 {
        return None;
    }
    let (negative, unsigned) = amount_text
        .strip_prefix('-')
        .map_or((false, amount_text), |unsigned_text| (true, unsigned_text));
    let scaled_amount =
        i128::from(crate::pricing::decimal_to_scaled_allow_zero(unsigned, scale).ok()?);
    i64::try_from(if negative {
        -scaled_amount
    } else {
        scaled_amount
    })
    .ok()
}

fn divide_rounded(numerator: i128, divisor: i128) -> Option<i64> {
    let magnitude = numerator.abs();
    let rounded = magnitude / divisor + i128::from(magnitude % divisor >= (divisor + 1) / 2);
    i64::try_from(if numerator < 0 { -rounded } else { rounded }).ok()
}
