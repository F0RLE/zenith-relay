use super::MAX_IDLE_CONNECTIONS_PER_HOST;
use crate::protocol::ClientWireApi;
use crate::sources::{is_http_endpoint, is_loopback_url, url_has_userinfo};
use crate::{
    Error, ModelRules, ProxyConfig, Result, RuntimeCandidate, RuntimeCandidatePolicy, WireApi,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use url::Url;

pub(in crate::runtime) fn normalize_client_wire_api(wire_api: ClientWireApi) -> ClientWireApi {
    // Older key records could carry `images` as if it were an independent
    // client protocol. Image requests are authorized by the
    // Chat-Completions-compatible surface, so retain backward compatibility
    // without exposing a dead standalone scope.
    match wire_api {
        ClientWireApi::Images => ClientWireApi::ChatCompletions,
        other => other,
    }
}

pub(in crate::runtime) fn all_native_wire_apis() -> Vec<WireApi> {
    vec![
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
        WireApi::Gemini,
    ]
}

pub(in crate::runtime) fn client_wire_apis_to_native(
    client_wire_apis: &[ClientWireApi],
) -> Vec<WireApi> {
    client_wire_apis
        .iter()
        .map(|wire_api| match wire_api {
            ClientWireApi::Responses => WireApi::Responses,
            ClientWireApi::ChatCompletions | ClientWireApi::Images => WireApi::ChatCompletions,
            ClientWireApi::Messages => WireApi::Messages,
            ClientWireApi::Gemini => WireApi::Gemini,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(in crate::runtime) fn runtime_now_ms() -> u64 {
    crate::unix_time_ms()
}

pub(in crate::runtime) fn basis_points_headers(account_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(account_header_value) = HeaderValue::from_str(account_id) {
        let mut account_header_value = account_header_value;
        account_header_value.set_sensitive(true);
        headers.insert(
            HeaderName::from_static("x-openai-account-id"),
            account_header_value.clone(),
        );
        headers.insert(
            HeaderName::from_static("chatgpt-account-id"),
            account_header_value,
        );
    }
    for (header_name, header_value) in [
        ("x-basispoints-auth-mode", "chatgpt"),
        ("origin", "https://bps.openai.com"),
        (
            "x-openai-internal-basispoints-client-agent-profile",
            "excel",
        ),
        ("x-openai-internal-basispoints-client-editor", "excel"),
        ("x-openai-internal-basispoints-client-host", "office"),
        ("x-openai-internal-basispoints-client-platform", "excel"),
        ("x-openai-internal-basispoints-client-platform-class", "PC"),
        (
            "x-openai-internal-basispoints-client-product",
            "basispoints-excel-plugin",
        ),
        ("x-openai-internal-basispoints-client-runtime", "desktop"),
        ("x-openai-internal-basispoints-office-host", "Excel"),
        ("x-openai-internal-basispoints-office-platform", "PC"),
        ("x-openai-internal-basispoints-browser-name", "Chrome"),
        (
            "x-openai-internal-basispoints-browser-ua-brands",
            "\"Chromium\";v=\"154\", \"Google Chrome\";v=\"154\", \"Not A(Brand\";v=\"99\"",
        ),
        (
            "x-openai-internal-basispoints-browser-ua-mobile",
            "?0",
        ),
        (
            "x-openai-internal-basispoints-browser-ua-platform",
            "\"Windows\"",
        ),
        ("x-stainless-arch", "unknown"),
        ("x-stainless-lang", "js"),
        ("x-stainless-os", "Unknown"),
        ("x-stainless-package-version", "7.25.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "browser:chrome"),
        ("x-stainless-runtime-version", "154.0.0"),
        (
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36",
        ),
    ] {
        headers.insert(
            HeaderName::from_static(header_name),
            HeaderValue::from_static(header_value),
        );
    }
    headers.insert(
        HeaderName::from_static("accept-encoding"),
        HeaderValue::from_static("identity"),
    );
    headers
}

pub(in crate::runtime) fn parse_bearer(authorization_header: &str) -> Option<&str> {
    let (scheme, secret) = authorization_header
        .trim()
        .split_once(char::is_whitespace)?;
    let secret = secret.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !secret.is_empty()).then_some(secret)
}

pub(in crate::runtime) fn normalized_set<'a>(
    model_ids: impl IntoIterator<Item = &'a String>,
) -> BTreeSet<String> {
    let mut normalized = BTreeMap::new();
    for model_id in model_ids {
        let model_id = model_id.trim();
        if !model_id.is_empty() {
            normalized
                .entry(crate::model_id_key(model_id))
                .or_insert_with(|| model_id.to_string());
        }
    }
    normalized.into_values().collect()
}

pub(in crate::runtime) fn model_rules(allowed: &[String], excluded: &[String]) -> ModelRules {
    ModelRules {
        allowed: normalized_set(allowed.iter()),
        excluded: normalized_set(excluded.iter()),
    }
}

pub(in crate::runtime) fn apply_candidate_policy(
    candidate: &mut RuntimeCandidate,
    policy: &RuntimeCandidatePolicy,
    rules: &ModelRules,
) {
    candidate.enabled = policy.enabled;
    candidate.draining = policy.draining;
    candidate.priority = policy.priority;
    candidate.weight = policy.weight;
    candidate.model_rules = rules.clone();
}

pub(in crate::runtime) fn normalize_prefix(prefix: Option<String>) -> Option<String> {
    prefix
        .map(|prefix_value| prefix_value.trim().trim_matches('/').to_string())
        .filter(|prefix_value| !prefix_value.is_empty())
}

pub(in crate::runtime) fn normalized_responses_url(responses_url: &str) -> Result<Url> {
    let url = Url::parse(responses_url.trim())
        .map_err(|_| Error::Validation("account Responses URL is invalid".to_string()))?;
    if !is_http_endpoint(&url) {
        return Err(Error::Validation(
            "account Responses URL must use HTTP or HTTPS".to_string(),
        ));
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        return Err(Error::Validation(
            "unencrypted account Responses URLs are allowed only on loopback".to_string(),
        ));
    }
    if url_has_userinfo(&url) || url.query().is_some() || url.fragment().is_some() {
        return Err(Error::Validation(
            "account Responses URL must not contain credentials, query, or fragment".to_string(),
        ));
    }
    Ok(url)
}

pub(in crate::runtime) fn runtime_client_builder(
    proxy: Option<&ProxyConfig>,
) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(MAX_IDLE_CONNECTIONS_PER_HOST)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::none());
    match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
}

pub(in crate::runtime) fn runtime_client(proxy: Option<&ProxyConfig>) -> Result<reqwest::Client> {
    // A quiet or long generation is still an active request. Reqwest's default
    // has no response/read deadline; retain only the connection timeout above.
    // Metadata and credential operations set their own request-level timeout.
    runtime_client_builder(proxy)
        .http2_adaptive_window(true)
        .build()
        .map_err(Error::from)
}

pub(in crate::runtime) fn runtime_websocket_client(
    proxy: Option<&ProxyConfig>,
) -> Result<reqwest::Client> {
    runtime_client_builder(proxy)
        .http1_only()
        .build()
        .map_err(Error::from)
}

pub(in crate::runtime) fn require_runtime_value(
    setting_name: &str,
    runtime_value: &str,
) -> Result<()> {
    if runtime_value.trim().is_empty() {
        Err(Error::Validation(format!(
            "{setting_name} must not be empty"
        )))
    } else {
        Ok(())
    }
}

pub(in crate::runtime) fn strip_prefix_ignore_ascii_case<'a>(
    input_text: &'a str,
    prefix: &str,
) -> Option<&'a str> {
    input_text
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .then(|| &input_text[prefix.len()..])
}
