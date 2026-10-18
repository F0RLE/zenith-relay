use super::*;

// Official field filtering returns only exit metadata, never an account lookup.
// https://ipwhois.io/documentation — anonymous HTTPS, timezone.id is IANA.
const LOCATION_URL: &str = "https://ipwho.is/?fields=success,ip,country_code,timezone.id";

pub(super) struct ProxyLocation {
    pub ip: IpAddr,
    pub country_code: Option<String>,
    pub timezone: Option<String>,
}

pub(super) async fn lookup(
    proxy: &ProxyConfig,
    timeout: Duration,
) -> Result<ProxyLocation, &'static str> {
    lookup_at(proxy, LOCATION_URL, CHECK_URL, timeout).await
}

async fn lookup_at(
    proxy: &ProxyConfig,
    location_url: &str,
    trace_url: &str,
    timeout: Duration,
) -> Result<ProxyLocation, &'static str> {
    let started = Instant::now();
    let location = proxy_check_body(proxy, location_url, timeout.min(Duration::from_secs(6)))
        .await
        .and_then(|body| parse_location(&body));
    match location {
        Ok(location) => Ok(location),
        Err(error_codes::PROXY_CHECK_AUTH_FAILED) => Err(error_codes::PROXY_CHECK_AUTH_FAILED),
        Err(error) => {
            // Geolocation availability is not proxy health. A bounded trace
            // fallback can still confirm connectivity, always via this proxy.
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(error);
            }
            let (ip, country_code) = proxy_check_request(proxy, trace_url, remaining).await?;
            Ok(ProxyLocation {
                ip,
                country_code,
                timezone: None,
            })
        }
    }
}

fn parse_location(body: &[u8]) -> Result<ProxyLocation, &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| error_codes::PROXY_CHECK_INVALID_RESPONSE)?;
    if value["success"] != true {
        return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
    }
    let ip = value["ip"]
        .as_str()
        .and_then(|ip| ip.parse().ok())
        .ok_or(error_codes::PROXY_CHECK_INVALID_RESPONSE)?;
    let country_code = value["country_code"]
        .as_str()
        .filter(|country| {
            country.len() == 2
                && *country != "XX"
                && country.bytes().all(|byte| byte.is_ascii_uppercase())
        })
        .map(str::to_owned);
    let timezone = value
        .pointer("/timezone/id")
        .and_then(serde_json::Value::as_str)
        .and_then(|timezone| {
            zenith_relay_core::normalize_timezone(Some(timezone.to_owned()))
                .ok()
                .flatten()
        });
    Ok(ProxyLocation {
        ip,
        country_code,
        timezone,
    })
}

#[cfg(test)]
mod tests;
