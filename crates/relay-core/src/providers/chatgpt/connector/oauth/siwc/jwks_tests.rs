use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn keys() -> JwkSet {
    serde_json::from_value(serde_json::json!({"keys":[{
        "kid":"synthetic", "kty":"RSA", "n":"AQAB", "e":"AQAB"
    }]}))
    .unwrap()
}

#[tokio::test]
async fn siwc_jwks_concurrent_reads_fetch_once() {
    let cache = Cache::default();
    let count = AtomicUsize::new(0);
    let fetch = || async {
        count.fetch_add(1, Ordering::Relaxed);
        tokio::task::yield_now().await;
        Ok(keys())
    };
    let now = Instant::now();
    let (a, b) = tokio::join!(
        cache.get("synthetic", now, fetch),
        cache.get("synthetic", now, fetch)
    );
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn siwc_jwks_rotation_refresh_is_bounded() {
    let cache = Cache::default();
    let now = Instant::now();
    cache
        .get("synthetic", now, || async { Ok(keys()) })
        .await
        .unwrap();
    assert!(
        cache
            .get("rotated", now, || async {
                panic!("cooldown must suppress fetch")
            })
            .await
            .is_err()
    );
    let mut rotated = keys();
    rotated.keys[0].common.key_id = Some("rotated".into());
    cache
        .get("rotated", now + RETRY_INTERVAL, || async { Ok(rotated) })
        .await
        .unwrap();
    assert!(
        cache
            .get("missing", now + RETRY_INTERVAL, || async {
                panic!("unknown key storm")
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn siwc_jwks_outage_preserves_fresh_keys_but_never_accepts_stale_keys() {
    let cache = Cache::default();
    let now = Instant::now();
    cache
        .get("synthetic", now, || async { Ok(keys()) })
        .await
        .unwrap();
    let error = cache
        .get("unknown", now + RETRY_INTERVAL, || async {
            Err(unavailable())
        })
        .await
        .unwrap_err();
    assert!(error.retryable);
    assert_eq!(
        error.provider_code.as_deref(),
        Some("siwc_identity_verification_unavailable")
    );
    cache
        .get("synthetic", now + RETRY_INTERVAL, || async {
            panic!("fresh key remains usable")
        })
        .await
        .unwrap();
    assert!(
        cache
            .get("synthetic", now + TTL, || async { Err(unavailable()) })
            .await
            .unwrap_err()
            .retryable
    );
    assert!(
        cache
            .get("synthetic", now + TTL, || async {
                panic!("outage cooldown")
            })
            .await
            .unwrap_err()
            .retryable
    );
}
