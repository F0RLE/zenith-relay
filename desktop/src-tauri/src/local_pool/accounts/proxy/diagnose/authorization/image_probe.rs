//! Explicit single-send research; never changes the working pool or credentials.
use super::{eligible, load_credentials, load_records, same_registration};
use crate::local_pool::accounts::credentials::StoredCodexCredentials;
use crate::local_pool::models::LocalAccountRecord;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};
use zenith_relay_core::{
    ProxyConfig,
    providers::chatgpt::{OAuthClientKind, basis_points_headers},
    unix_time_ms,
};

const FLAG: &str = "--probe-bps-images";
const ENDPOINT: &str = "https://bps.openai.com/basispoints/api/images/generations";
const MAX_BODY_BYTES: usize = 128 * 1024 * 1024;

mod preview;

pub(in crate::local_pool::accounts::proxy::diagnose) fn requested(args: &[OsString]) -> bool {
    args.iter().any(|arg| arg == FLAG)
}

pub(in crate::local_pool::accounts::proxy::diagnose) fn run(
    args: &[OsString],
) -> Result<Value, &'static str> {
    let (args, preview) = preview::arguments(args)?;
    let (root, size, expected) = arguments(args)?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "runtime_unavailable")?
        .block_on(probe(root, size, expected, preview.as_ref()))
}

fn arguments(args: &[OsString]) -> Result<(&Path, &str, Option<&str>), &'static str> {
    if !(args.len() == 5 || args.len() == 6)
        || args[1] != FLAG
        || args[3] != "--allow-generation"
        || !Path::new(&args[2]).is_absolute()
    {
        return Err(
            "expected_root_explicit_generation_consent_size_and_optional_account_reference",
        );
    }
    let size = args[4]
        .to_str()
        .filter(|size| matches!(*size, "1024x1024" | "2048x2048" | "3840x2160"))
        .ok_or("unsupported_probe_size")?;
    let expected = args
        .get(5)
        .map(|value| {
            value
                .to_str()
                .filter(|value| {
                    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .ok_or("invalid_account_reference")
        })
        .transpose()?;
    Ok((Path::new(&args[2]), size, expected))
}

fn reference(id: &str) -> String {
    Sha256::digest(id.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Selected {
    record: LocalAccountRecord,
    credentials: StoredCodexCredentials,
    proxy_url: Option<String>,
}

fn select(root: &Path, expected: Option<&str>) -> Result<Selected, &'static str> {
    for record in load_records(root)?.into_iter().filter(eligible) {
        if record.oauth_client_kind != Some(OAuthClientKind::ExcelBps)
            || expected.is_some_and(|expected| reference(&record.account.id) != expected)
        {
            continue;
        }
        let Ok(credentials) = load_credentials(root, &record.account.id) else {
            continue;
        };
        if credentials.oauth_client_kind() != OAuthClientKind::ExcelBps || !credentials.has_oauth()
        {
            continue;
        }
        let tokens = credentials
            .to_token_set()
            .map_err(|_| "credential_invalid")?;
        if !tokens.is_access_usable(unix_time_ms(), 300_000)
            || record.account.token_generation != tokens.generation()
        {
            continue;
        }
        // Do not replace an assigned route with a common proxy or environment fallback.
        let proxy_url = credentials.proxy_url().map(str::to_owned);
        if proxy_url.is_none() && !credentials.bypass_common_proxy() {
            continue;
        }
        return Ok(Selected {
            record,
            credentials,
            proxy_url,
        });
    }
    Err("eligible_excel_account_with_fresh_credentials_and_explicit_proxy_route_required")
}

fn still_current(root: &Path, selected: &Selected) -> bool {
    let Ok(records) = load_records(root) else {
        return false;
    };
    records.iter().any(|record| {
        eligible(record)
            && record.account.id == selected.record.account.id
            && same_registration(record, &selected.record)
            && load_credentials(root, &record.account.id)
                .is_ok_and(|credentials| credentials.matches_snapshot(&selected.credentials))
    })
}

fn payload(size: &str) -> Value {
    json!({"model":"gpt-image-2","prompt":"A simple red ceramic cube on a plain white background. No text, no people, no logos.",
        "size":size,"quality":"high","output_format":"png","background":"opaque","n":1})
}

async fn probe(
    root: &Path,
    size: &str,
    expected: Option<&str>,
    preview: Option<&preview::Preview>,
) -> Result<Value, &'static str> {
    let selected = select(root, expected)?;
    let tokens = selected
        .credentials
        .to_token_set()
        .map_err(|_| "credential_invalid")?;
    let account_id = selected
        .credentials
        .provider_account_id()
        .ok_or("account_identity_missing")?;
    let headers = basis_points_headers(
        account_id,
        selected.credentials.provider_user_id(),
        selected.credentials.basis_points_headers(),
    );
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(240));
    if let Some(proxy) = selected.proxy_url.as_deref() {
        builder = ProxyConfig::parse(proxy)
            .map_err(|_| "proxy_configuration_invalid")?
            .apply(builder);
    }
    let client = builder.build().map_err(|_| "client_unavailable")?;
    if !still_current(root, &selected) {
        return Err("account_changed_before_dispatch");
    }
    let started = Instant::now();
    let mut result = json!({"route":"excel_bps_images","accountRef":reference(&selected.record.account.id),
        "requestedModel":"gpt-image-2","requestedSize":size,"requestedQuality":"high",
        "requests":1,"retries":0,"poolUsageRecorded":false,"workingConfigurationChanged":false});
    let mut request = payload(size);
    if let Some(preview) = preview {
        request["prompt"] = json!(preview.prompt);
    }
    let response = client
        .post(ENDPOINT)
        .headers(headers)
        .bearer_auth(tokens.access_token())
        .json(&request)
        .send()
        .await;
    let mut response = match response {
        Ok(response) => response,
        Err(_) => {
            result["status"] = json!("execution_uncertain");
            result["stage"] = json!("response_headers");
            return Ok(result);
        }
    };
    result["httpStatus"] = json!(response.status().as_u16());
    let success_status = response.status().is_success();
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len().saturating_add(chunk.len()) <= MAX_BODY_BYTES => {
                bytes.extend_from_slice(&chunk)
            }
            Ok(None) => break,
            _ => {
                result["status"] = json!("execution_uncertain");
                result["stage"] = json!("response_body");
                return Ok(result);
            }
        }
    }
    result["elapsedMs"] = json!(started.elapsed().as_millis() as u64);
    result["responseBytes"] = json!(bytes.len());
    let parsed = serde_json::from_slice::<Value>(&bytes).ok();
    result["jsonResponse"] = json!(parsed.is_some());
    if !success_status {
        result["status"] = json!("rejected");
        result["error"] = json!(
            zenith_relay_core::usage::UpstreamErrorDetails::from_response_body(
                result["httpStatus"].as_u64().map(|status| status as u16),
                &bytes
            )
            .sanitized()
        );
        return Ok(result);
    }
    let Some(parsed) = parsed else {
        result["status"] = json!("execution_uncertain");
        return Ok(result);
    };
    let images = parsed.get("data").and_then(Value::as_array);
    result["outputCount"] = json!(images.map(Vec::len));
    let dimensions = images
        .and_then(|images| images.first())
        .and_then(|image| image.get("b64_json"))
        .and_then(Value::as_str)
        .and_then(png_dimensions);
    result["dimensions"] = dimensions.map(|(width,height)| json!({"width":width,"height":height,"pixels":u64::from(width)*u64::from(height)})).unwrap_or(Value::Null);
    result["status"] = json!(response_outcome(&parsed, dimensions));
    if parsed.get("error").is_some_and(|error| !error.is_null()) {
        result["error"] = json!(
            zenith_relay_core::usage::UpstreamErrorDetails::from_response_body(Some(200), &bytes)
                .sanitized()
        );
    }
    let options: zenith_relay_core::usage::ImageOptionsDiagnostics = serde_json::from_value(json!({
        "model":parsed.get("model").and_then(Value::as_str),"size":parsed.get("size").and_then(Value::as_str),
        "quality":parsed.get("quality").and_then(Value::as_str),"outputFormat":parsed.get("output_format").and_then(Value::as_str)
    })).map_err(|_| "metadata_invalid")?;
    result["reported"] = json!(
        zenith_relay_core::usage::ImageDiagnostics {
            requested: options,
            ..Default::default()
        }
        .sanitized()
        .requested
    );
    for key in ["input_tokens", "output_tokens", "total_tokens"] {
        result[key] = json!(
            parsed
                .get("usage")
                .and_then(|usage| usage.get(key))
                .and_then(Value::as_u64)
        );
    }
    if let Some(preview) = preview
        && result["status"] == "completed"
    {
        // Saving failure must not turn a completed generation into permission to retry.
        match preview.save(&parsed) {
            Ok(bytes) => {
                result["outputFile"] = json!(preview.output);
                result["outputBytes"] = json!(bytes);
            }
            Err(code) => result["outputError"] = json!(code),
        }
    }
    Ok(result)
}

fn response_outcome(parsed: &Value, dimensions: Option<(u32, u32)>) -> &'static str {
    if parsed.get("error").is_some_and(|error| !error.is_null()) {
        "provider_error"
    } else if dimensions.is_some()
        && parsed
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|images| images.len() == 1)
    {
        "completed"
    } else {
        "output_unverified"
    }
}

fn png_dimensions(encoded: &str) -> Option<(u32, u32)> {
    let prefix = STANDARD.decode(encoded.get(..32)?).ok()?;
    if prefix.get(..16)? != b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR" {
        return None;
    }
    let width = u32::from_be_bytes(prefix.get(16..20)?.try_into().ok()?);
    let height = u32::from_be_bytes(prefix.get(20..24)?.try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_probe_requires_explicit_consent_and_bounded_size() {
        let root = std::env::temp_dir().into_os_string();
        let mut args = vec![
            "relay".into(),
            FLAG.into(),
            root,
            "--allow-generation".into(),
            "1024x1024".into(),
        ];
        assert!(arguments(&args).is_ok());
        args[3] = "--implicit".into();
        assert!(arguments(&args).is_err());
        args[3] = "--allow-generation".into();
        args[4] = "arbitrary".into();
        assert!(arguments(&args).is_err());
    }
    #[test]
    fn image_probe_changes_only_size_and_reads_actual_png_header() {
        let mut a = payload("1024x1024");
        a["size"] = json!("3840x2160");
        assert_eq!(a, payload("3840x2160"));
        assert_eq!(a["n"], 1);
        assert!(a.get("stream").is_none());
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1672_u32.to_be_bytes());
        png.extend_from_slice(&941_u32.to_be_bytes());
        assert_eq!(png_dimensions(&STANDARD.encode(png)), Some((1672, 941)));
        assert_eq!(png_dimensions("not an image"), None);
    }

    #[test]
    fn image_probe_does_not_mark_partial_error_or_extra_output_as_completed() {
        assert_eq!(
            response_outcome(&json!({"data":[{}]}), Some((1024, 1024))),
            "completed"
        );
        assert_eq!(
            response_outcome(
                &json!({"data":[{}],"error":{"code":"synthetic"}}),
                Some((1024, 1024))
            ),
            "provider_error"
        );
        assert_eq!(
            response_outcome(&json!({"data":[{},{}]}), Some((1024, 1024))),
            "output_unverified"
        );
        assert_eq!(
            response_outcome(&json!({"data":[{}]}), None),
            "output_unverified"
        );
    }
}
