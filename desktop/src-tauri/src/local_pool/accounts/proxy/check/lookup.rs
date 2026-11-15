use super::{Duration, IpAddr, ProxyConfig, proxy_check_body};

pub(super) const BASE_URL: &str = "https://ipwho.is/";

pub(super) struct ProxyLocation {
    pub country_code: String,
    pub region: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
}

/// Enrich the already observed exit, never the hostname or a second rotating exit.
pub(super) async fn lookup(
    proxy: &ProxyConfig,
    ip: IpAddr,
    observed_country: Option<&str>,
    base_url: &str,
    timeout: Duration,
) -> Result<ProxyLocation, &'static str> {
    let url = format!("{base_url}{ip}?fields=success,ip,country_code,region,city,timezone.id");
    let body = tokio::time::timeout(timeout, proxy_check_body(proxy, &url, timeout))
        .await
        .map_err(|_| "proxy_location_unavailable")?
        .map_err(|_| "proxy_location_unavailable")?;
    parse_location(&body, ip, observed_country)
}

fn parse_location(
    body: &[u8],
    expected_ip: IpAddr,
    observed_country: Option<&str>,
) -> Result<ProxyLocation, &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| "proxy_location_unavailable")?;
    if value["success"] != true
        || value["ip"]
            .as_str()
            .and_then(|ip| ip.parse::<IpAddr>().ok())
            != Some(expected_ip)
    {
        return Err("proxy_location_unavailable");
    }
    let country_code = value["country_code"]
        .as_str()
        .filter(|country| {
            country.len() == 2
                && *country != "XX"
                && country.bytes().all(|byte| byte.is_ascii_uppercase())
        })
        .ok_or("proxy_location_unavailable")?;
    // Do not combine a country from the probe with a conflicting city/timezone.
    if observed_country.is_some_and(|country| country != country_code) {
        return Err("proxy_location_mismatch");
    }
    let timezone = value
        .pointer("/timezone/id")
        .and_then(serde_json::Value::as_str)
        .and_then(|timezone| {
            zenith_relay_core::normalize_timezone(Some(timezone.into()))
                .ok()
                .flatten()
        });
    Ok(ProxyLocation {
        country_code: country_code.into(),
        region: place(&value["region"]),
        city: place(&value["city"]),
        timezone,
    })
}

fn place(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;
