use cookie_store::{Cookie, CookieStore};
use reqwest::header::{HeaderMap, HeaderValue, COOKIE, SET_COOKIE};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use url::Url;

/// Per-executor, memory-only infrastructure state. An in-flight response keeps
/// its original jar, so a late response cannot populate a new credential's jar.
#[derive(Default)]
pub(super) struct RoutingCookies {
    current: Mutex<Option<([u8; 32], Arc<RoutingCookieJar>)>>,
}

impl RoutingCookies {
    pub(super) fn for_credential(&self, credential: &[u8]) -> Arc<RoutingCookieJar> {
        let owner: [u8; 32] = Sha256::digest(credential).into();
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((previous, jar)) = current.as_ref() {
            if *previous == owner {
                return jar.clone();
            }
        }
        let jar = Arc::new(RoutingCookieJar::default());
        *current = Some((owner, jar.clone()));
        jar
    }
}

#[derive(Default)]
pub(crate) struct RoutingCookieJar {
    store: Mutex<CookieStore>,
}

fn allowed_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some("chatgpt.com")
        && url.port_or_known_default() == Some(443)
        && (url.path() == "/backend-api" || url.path().starts_with("/backend-api/"))
}

impl RoutingCookieJar {
    pub(crate) fn apply(&self, url: &Url, headers: &mut HeaderMap) {
        headers.remove(COOKIE);
        if !allowed_url(url) {
            return;
        }
        let store = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let value = store
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if !value.is_empty() {
            if let Ok(mut value) = HeaderValue::from_str(&value) {
                value.set_sensitive(true);
                headers.insert(COOKIE, value);
            }
        }
    }

    pub(crate) fn observe(&self, url: &Url, headers: &HeaderMap) {
        if !allowed_url(url) {
            return;
        }
        let mut store = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for value in headers.get_all(SET_COOKIE) {
            if value.as_bytes().len() > 4096 {
                continue;
            }
            let Ok(value) = value.to_str() else { continue };
            let Ok(cookie) = Cookie::parse(value.to_owned(), url) else {
                continue;
            };
            if cookie.name() != "__oailb" || !matches!(cookie.domain(), None | Some("chatgpt.com"))
            {
                continue;
            }
            let existing = store
                .iter_any()
                .any(|entry| entry.domain == cookie.domain && entry.path == cookie.path);
            if existing || store.iter_any().count() < 16 {
                let _ = store.insert(cookie, url);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://chatgpt.com{path}")).unwrap()
    }

    fn observe(jar: &RoutingCookieJar, value: &str) {
        let mut headers = HeaderMap::new();
        headers.insert(SET_COOKIE, HeaderValue::from_str(value).unwrap());
        jar.observe(&url("/backend-api/codex/responses"), &headers);
    }

    fn sent(jar: &RoutingCookieJar, url: &Url) -> Option<HeaderValue> {
        let mut headers = HeaderMap::new();
        jar.apply(url, &mut headers);
        headers.remove(COOKIE)
    }

    #[test]
    fn routing_cookie_obeys_origin_path_expiry_and_allowlist() {
        let jar = RoutingCookieJar::default();
        observe(&jar, "session=synthetic; Path=/; Secure");
        observe(
            &jar,
            "__oailb=synthetic-route; Path=/backend-api; Max-Age=3600; Secure; HttpOnly",
        );
        let value = sent(&jar, &url("/backend-api/codex/models")).unwrap();
        assert_eq!(value, "__oailb=synthetic-route");
        assert!(value.is_sensitive());
        for target in [
            "https://chatgpt.com/",
            "https://chatgpt.com/backend-api-other",
            "http://chatgpt.com/backend-api/codex",
            "https://api.openai.com/backend-api/codex",
            "https://sub.chatgpt.com/backend-api/codex",
            "https://chatgpt.com:444/backend-api/codex",
        ] {
            assert!(sent(&jar, &Url::parse(target).unwrap()).is_none());
        }
        observe(
            &jar,
            "__oailb=deleted; Path=/backend-api; Max-Age=0; Secure",
        );
        assert!(sent(&jar, &url("/backend-api/codex/models")).is_none());
        observe(
            &jar,
            "__oailb=expired; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure",
        );
        observe(&jar, "__oailb=foreign; Domain=example.com; Path=/; Secure");
        assert!(sent(&jar, &url("/backend-api/codex/models")).is_none());
    }

    #[test]
    fn accounts_credentials_and_late_responses_are_isolated() {
        let account = RoutingCookies::default();
        let other_account = RoutingCookies::default();
        let old = account.for_credential(b"synthetic-old");
        observe(&old, "__oailb=old; Path=/; Secure");
        assert!(Arc::ptr_eq(&old, &account.for_credential(b"synthetic-old")));
        let new = account.for_credential(b"synthetic-new");
        observe(&old, "__oailb=late; Path=/; Secure");
        assert!(sent(&new, &url("/backend-api/codex/models")).is_none());
        assert!(sent(
            &other_account.for_credential(b"synthetic-old"),
            &url("/backend-api/codex/models")
        )
        .is_none());
        observe(&new, "__oailb=new; Path=/; Secure");
        assert_eq!(
            sent(
                &account.for_credential(b"synthetic-new"),
                &url("/backend-api/codex/models")
            )
            .unwrap(),
            "__oailb=new"
        );
    }
}
