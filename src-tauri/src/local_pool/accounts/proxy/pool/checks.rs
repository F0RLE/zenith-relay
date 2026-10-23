use super::*;

fn checked(proxy_id: &str, time: u64, ip: &str, timezone: Option<&str>) -> ProxyCheckResult {
    ProxyCheckResult {
        proxy_id: proxy_id.into(),
        checked_at_ms: time,
        elapsed_ms: 5,
        ip: Some(ip.parse().unwrap()),
        country_code: Some("NL".into()),
        timezone: timezone.map(str::to_owned),
        error_code: None,
        ..ProxyCheckResult::default()
    }
}

#[test]
fn checks_persist_valid_zones_and_reject_stale_deleted_or_replaced_proxies() {
    let mut pool = ProxyPool::default();
    pool.import(&["http://proxy.example:8080".into()], 1)
        .unwrap();
    let id = pool.summary().entries[0].id.clone();
    let url = pool.stored_url(&id).unwrap();
    assert!(pool.record_check(
        &url,
        checked(&id, 10, "203.0.113.9", Some(" Europe/Amsterdam "))
    ));
    assert_eq!(
        pool.timezone(&id).unwrap().as_deref(),
        Some("Europe/Amsterdam")
    );
    assert!(!pool.record_check(&url, checked(&id, 9, "203.0.113.9", Some("Asia/Tokyo"))));
    let restored = ProxyPool::from_json(&serde_json::to_string(&pool).unwrap()).unwrap();
    assert_eq!(restored.summary(), pool.summary());
    pool.entries[0].url = "http://replacement.example:8080/".into();
    assert!(!pool.record_check(&url, checked(&id, 20, "203.0.113.9", Some("Asia/Tokyo"))));
    pool.entries.clear();
    assert!(!pool.record_check(&url, checked(&id, 30, "203.0.113.9", Some("Asia/Tokyo"))));
}

#[test]
fn failed_checks_keep_a_known_zone_but_a_changed_exit_without_a_zone_invalidates_it() {
    let mut pool = ProxyPool::default();
    pool.import(&["http://proxy.example:8080".into()], 1)
        .unwrap();
    let id = pool.summary().entries[0].id.clone();
    let url = pool.stored_url(&id).unwrap();
    pool.record_check(
        &url,
        checked(&id, 10, "203.0.113.9", Some("Europe/Amsterdam")),
    );
    pool.record_check(
        &url,
        checked(&id, 20, "203.0.113.9", Some("Invalid/Timezone")),
    );
    assert_eq!(
        pool.timezone(&id).unwrap().as_deref(),
        Some("Europe/Amsterdam")
    );
    assert_eq!(
        pool.summary().entries[0]
            .last_check
            .as_ref()
            .unwrap()
            .timezone,
        None
    );
    let mut failed = checked(&id, 30, "203.0.113.10", Some("Asia/Tokyo"));
    failed.error_code = Some("proxy_check_timeout".into());
    failed.ip = None;
    pool.record_check(&url, failed);
    assert_eq!(
        pool.timezone(&id).unwrap().as_deref(),
        Some("Europe/Amsterdam")
    );
    pool.record_check(&url, checked(&id, 40, "203.0.113.10", None));
    assert_eq!(pool.timezone(&id).unwrap(), None);
}
