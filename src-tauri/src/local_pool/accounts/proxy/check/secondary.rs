use super::{lookup, proxy_check_body, Duration, Instant, IpAddr, ProxyCheckResult, ProxyConfig};

/// Supplemental probing shares the selected proxy and never retries directly.
/// Its total budget includes geolocation and cannot inflate the displayed ping.
pub(super) async fn probe(
    proxy: &ProxyConfig,
    expect_ipv4: bool,
    endpoint: &str,
    location_base: &str,
) -> Result<(IpAddr, Result<lookup::ProxyLocation, &'static str>), &'static str> {
    let budget = Duration::from_secs(3);
    let started = Instant::now();
    let body = tokio::time::timeout(budget, proxy_check_body(proxy, endpoint, budget))
        .await
        .map_err(|_| "proxy_secondary_unavailable")?
        .map_err(|_| "proxy_secondary_unavailable")?;
    let ip = std::str::from_utf8(&body)
        .ok()
        .and_then(|text| text.trim().parse::<IpAddr>().ok())
        .filter(|ip| ip.is_ipv4() == expect_ipv4)
        .ok_or("proxy_secondary_invalid_response")?;
    let location = lookup::lookup(
        proxy,
        ip,
        None,
        location_base,
        budget.saturating_sub(started.elapsed()),
    )
    .await;
    Ok((ip, location))
}

pub(super) fn set_address(result: &mut ProxyCheckResult, address: IpAddr) {
    match address {
        IpAddr::V4(ip) => result.ipv4 = Some(ip),
        IpAddr::V6(ip) => result.ipv6 = Some(ip),
    }
}

pub(super) fn check_location(result: &mut ProxyCheckResult, other: &lookup::ProxyLocation) {
    let country_conflict = result
        .country_code
        .as_ref()
        .is_some_and(|country| country != &other.country_code);
    let zone_conflict = result
        .timezone
        .as_ref()
        .zip(other.timezone.as_ref())
        .is_some_and(|(a, b)| a != b);
    let region_conflict = result
        .region
        .as_ref()
        .zip(other.region.as_ref())
        .is_some_and(|(a, b)| a != b);
    if country_conflict || zone_conflict || region_conflict {
        if country_conflict {
            result.country_code = None;
        }
        result.timezone = None;
        result.region = None;
        result.city = None;
        result.location_error_code = Some("proxy_location_conflict".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dual_stack_checks_use_the_selected_proxy_and_keep_primary_success() {
        let (proxy, server) = super::super::tests::mock_proxy().await;
        let result = super::super::check_at(
            "synthetic".into(),
            &proxy,
            1,
            "http://unreachable.invalid/trace",
            "http://unreachable.invalid/",
            Some((
                "http://unreachable.invalid/invalid",
                "http://unreachable.invalid/ipv6",
            )),
        )
        .await;
        assert_eq!(result.ipv4.unwrap().to_string(), "203.0.113.9");
        assert_eq!(result.ipv6.unwrap().to_string(), "2001:db8::1");
        assert_eq!(
            result.location_error_code.as_deref(),
            Some("proxy_location_conflict")
        );
        assert!(result.timezone.is_none());
        assert!(result.error_code.is_none());
        let unavailable = super::super::check_at(
            "synthetic".into(),
            &proxy,
            2,
            "http://unreachable.invalid/trace",
            "http://unreachable.invalid/",
            Some((
                "http://unreachable.invalid/invalid",
                "http://unreachable.invalid/invalid",
            )),
        )
        .await;
        assert!(unavailable.error_code.is_none());
        assert!(unavailable.ipv4.is_some());
        assert!(unavailable.ipv6.is_none());
        assert!(unavailable.secondary_check_error.is_some());
        assert_eq!(unavailable.country_code.as_deref(), Some("NL"));
        server.abort();
    }

    #[test]
    fn different_exit_regions_cannot_supply_a_single_timezone() {
        let mut result = ProxyCheckResult {
            country_code: Some("NL".into()),
            timezone: Some("Europe/Amsterdam".into()),
            region: Some("North Holland".into()),
            city: Some("Amsterdam".into()),
            ..Default::default()
        };
        let other = lookup::ProxyLocation {
            country_code: "US".into(),
            region: None,
            city: None,
            timezone: Some("America/New_York".into()),
        };
        check_location(&mut result, &other);
        assert!(result.country_code.is_none());
        assert!(result.timezone.is_none());
        assert!(result.city.is_none());
        assert_eq!(
            result.location_error_code.as_deref(),
            Some("proxy_location_conflict")
        );
        assert!(
            result.error_code.is_none(),
            "a location conflict does not mean the proxy is broken"
        );
    }
}
