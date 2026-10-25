//! Catalog version signals are refresh hints, never capability/access evidence.
use super::*;

const MIN_REFRESH_INTERVAL_MS: u64 = 60_000;
const MAX_ETAG_BYTES: usize = 512;

#[derive(Clone, Eq, PartialEq)]
pub(super) struct CatalogHint {
    credential: [u8; 32],
    etag: [u8; 32],
    scheduled_for_ms: u64,
    next_allowed_ms: u64,
}

pub(super) type CatalogHintCallback = Arc<dyn Fn(String, u64) -> bool + Send + Sync>;

pub(crate) fn native_codex_responses_url(url: &Url) -> bool {
    matches!(url.scheme(), "https" | "wss")
        && url.host_str() == Some("chatgpt.com")
        && url.port_or_known_default() == Some(443)
        && matches!(
            url.path().trim_end_matches('/'),
            "/backend-api/codex/responses" | "/backend-api/codex/v1/responses"
        )
}

fn next_hint(
    previous: Option<&CatalogHint>,
    credential: [u8; 32],
    etag: &[u8],
    now_ms: u64,
) -> Option<(CatalogHint, u64)> {
    if etag.is_empty() || etag.len() > MAX_ETAG_BYTES || !etag.is_ascii() {
        return None;
    }
    let etag = Sha256::digest(etag).into();
    let previous = previous.filter(|previous| previous.credential == credential);
    if previous.is_some_and(|previous| previous.etag == etag) {
        return None;
    }
    let due_ms = previous.map_or(now_ms, |previous| {
        if previous.scheduled_for_ms > now_ms {
            previous.scheduled_for_ms
        } else {
            previous.next_allowed_ms.max(now_ms)
        }
    });
    Some((
        CatalogHint {
            credential,
            etag,
            scheduled_for_ms: due_ms,
            // Repeated changes inside the same window coalesce at the existing due
            // time, rather than indefinitely postponing or accelerating the read.
            next_allowed_ms: due_ms.saturating_add(MIN_REFRESH_INTERVAL_MS),
        },
        due_ms.saturating_sub(now_ms),
    ))
}

impl GatewayRuntime {
    pub fn set_codex_catalog_hint_callback(
        &self,
        callback: impl Fn(String, u64) -> bool + Send + Sync + 'static,
    ) {
        *crate::poison::mutex(&self.codex_catalog_hint_callback) = Arc::new(callback);
    }

    pub(crate) fn observe_codex_models_etag(
        &self,
        candidate_id: &str,
        url: &Url,
        headers: &HeaderMap,
        prepared: &PreparedAuthorization,
    ) {
        let Some(account) = self.chatgpt_accounts.get(candidate_id) else {
            return;
        };
        if account.oauth_client_kind != crate::providers::chatgpt::OAuthClientKind::Codex
            || !native_codex_responses_url(url)
            || prepared.dispatch_guard(self, candidate_id).is_none()
        {
            return;
        }
        let Some(etag) = headers.get("x-models-etag") else {
            return;
        };
        let (hint, delay_ms, previous) = {
            let mut hints = crate::poison::mutex(&self.codex_catalog_hints);
            let Some((hint, delay_ms)) = next_hint(
                hints.get(candidate_id),
                prepared.credential_fingerprint(),
                etag.as_bytes(),
                runtime_now_ms(),
            ) else {
                return;
            };
            // Entries can only be created for accounts in this runtime's bounded inventory.
            let previous = hints.insert(candidate_id.to_string(), hint.clone());
            (hint, delay_ms, previous)
        };
        let callback = crate::poison::mutex(&self.codex_catalog_hint_callback).clone();
        if !callback(account.id.clone(), delay_ms) {
            let mut hints = crate::poison::mutex(&self.codex_catalog_hints);
            if hints.get(candidate_id) == Some(&hint) {
                if let Some(previous) = previous {
                    hints.insert(candidate_id.to_string(), previous);
                } else {
                    hints.remove(candidate_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_coalesce_and_credential_changes_do_not_inherit_old_observations() {
        let (first, delay) = next_hint(None, [1; 32], b"v1", 1000).unwrap();
        assert_eq!(delay, 0);
        assert!(next_hint(Some(&first), [1; 32], b"v1", 2000).is_none());
        let (second, delay) = next_hint(Some(&first), [1; 32], b"v2", 2000).unwrap();
        assert_eq!(delay, 59_000);
        let (third, delay) = next_hint(Some(&second), [1; 32], b"v3", 3000).unwrap();
        assert_eq!(delay, 58_000);
        assert_eq!(
            next_hint(Some(&third), [1; 32], b"v4", 62_000).unwrap().1,
            59_000
        );
        assert_eq!(next_hint(Some(&first), [2; 32], b"v1", 2000).unwrap().1, 0);
    }

    #[test]
    fn malformed_tags_and_untrusted_routes_do_not_trigger_discovery() {
        for etag in [&b""[..], &vec![b'x'; MAX_ETAG_BYTES + 1], &[255]] {
            assert!(next_hint(None, [0; 32], etag, 0).is_none());
        }
        for url in [
            "https://example.test/backend-api/codex/responses",
            "http://chatgpt.com/backend-api/codex/responses",
            "https://chatgpt.com/backend-api/codex/responses/compact",
            "https://api.openai.com/v1/responses",
        ] {
            assert!(!native_codex_responses_url(&Url::parse(url).unwrap()));
        }
        assert!(native_codex_responses_url(
            &Url::parse("wss://chatgpt.com/backend-api/codex/responses").unwrap()
        ));
    }
}
