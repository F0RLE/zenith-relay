//! Upload user images before a Basis Points request.
//!
//! The upstream body accepts `input_image.file_id` only. A data URL is decoded,
//! checked by its bytes, and uploaded to the attachments endpoint beside the
//! responses URL. Remote URLs and extra fields such as `detail` are not forwarded.
//! Image bytes, file IDs and credentials stay out of errors and logs.

use super::super::super::errors::AttemptFailure;
use crate::runtime::AuthorizationIdentityPolicy;
use crate::GatewayRuntime;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

const MAX_UPLOAD_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_FILE_ID_LEN: usize = 512;
const MAX_ATTACHMENT_CACHE: usize = 512;

pub(in crate::gateway::execution) enum AttachmentFailure {
    Reject(AttemptFailure),
    Retry(AttemptFailure),
}

struct InlineImage {
    media_type: &'static str,
    filename: &'static str,
    bytes: Vec<u8>,
}

enum ImageJob {
    File(String),
    Upload(InlineImage),
}

struct LocatedJob {
    input_index: usize,
    content_index: usize,
    job: ImageJob,
}

struct AttachmentCache {
    order: VecDeque<[u8; 32]>,
    entries: HashMap<[u8; 32], String>,
}

pub(in crate::gateway::execution) async fn attach_input_images(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    responses_url: &url::Url,
    headers: &HeaderMap,
    request_body: Vec<u8>,
) -> Result<Vec<u8>, AttachmentFailure> {
    if !request_body
        .windows(13)
        .any(|window| window == b"\"input_image\"")
    {
        return Ok(request_body);
    }
    let mut request_fields: Value = match serde_json::from_slice(&request_body) {
        Ok(request_body) => request_body,
        Err(_) => return Ok(request_body),
    };
    let jobs = stage_input_images(&request_fields)?;
    if jobs.is_empty() {
        return Ok(request_body);
    }
    let endpoint = attachment_url(responses_url).map_err(|_| upload_failed())?;
    let credential_fingerprint = runtime
        .basis_points_credential_fingerprint(candidate_id)
        .await
        .unwrap_or_default();
    for job in &jobs {
        if let ImageJob::File(file_id) = &job.job {
            set_image_part(
                &mut request_fields,
                job.input_index,
                job.content_index,
                file_id,
            );
        }
    }
    for job in jobs {
        let ImageJob::Upload(image) = job.job else {
            continue;
        };
        let key = attachment_key(
            &endpoint,
            candidate_id,
            &credential_fingerprint,
            image.media_type,
            &image.bytes,
        );
        let file_id = if let Some(file_id) = cached_file_id(&key) {
            file_id
        } else {
            let file_id = upload_image(runtime, candidate_id, &endpoint, headers, &image).await?;
            remember_file_id(key, file_id.clone());
            file_id
        };
        set_image_part(
            &mut request_fields,
            job.input_index,
            job.content_index,
            &file_id,
        );
    }
    serde_json::to_vec(&request_fields).map_err(|_| upload_failed())
}

fn stage_input_images(request_fields: &Value) -> Result<Vec<LocatedJob>, AttachmentFailure> {
    let Some(input_items) = request_fields.get("input").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut jobs = Vec::new();
    for (input_index, input_item) in input_items.iter().enumerate() {
        let Some(input_item_fields) = input_item.as_object() else {
            continue;
        };
        let role = input_item_fields
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = input_item_fields
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if role != "user" || !(kind.is_empty() || kind == "message") {
            continue;
        }
        let Some(content_parts) = input_item_fields.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (content_index, content_part) in content_parts.iter().enumerate() {
            let Some(content_part_fields) = content_part.as_object() else {
                continue;
            };
            if content_part_fields.get("type").and_then(Value::as_str) != Some("input_image") {
                continue;
            }
            jobs.push(LocatedJob {
                input_index,
                content_index,
                job: image_job(content_part_fields)?,
            });
        }
    }
    Ok(jobs)
}

fn image_job(part: &Map<String, Value>) -> Result<ImageJob, AttachmentFailure> {
    let file_id = match part.get("file_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(file_id)) => {
            let file_id = file_id.trim();
            if file_id.is_empty() || file_id.len() > MAX_FILE_ID_LEN {
                return Err(invalid_image("input_image data URL is invalid"));
            }
            Some(file_id.to_string())
        }
        Some(_) => return Err(invalid_image("input_image data URL is invalid")),
    };
    let image_url = match part.get("image_url") {
        None | Some(Value::Null) => None,
        Some(Value::String(image_url)) => Some(image_url.as_str()),
        Some(Value::Object(image_url)) => match image_url.get("url") {
            None | Some(Value::Null) => None,
            Some(Value::String(image_url)) => Some(image_url.as_str()),
            Some(_) => return Err(invalid_image("input_image data URL is invalid")),
        },
        Some(_) => return Err(invalid_image("input_image data URL is invalid")),
    };
    let image_url = image_url
        .map(str::trim)
        .filter(|image_url| !image_url.is_empty());
    match (file_id, image_url) {
        (Some(_), Some(_)) => Err(invalid_image(
            "input_image cannot contain both image_url and file_id",
        )),
        (Some(file_id), None) => Ok(ImageJob::File(file_id)),
        (None, Some(image_url)) => {
            if !image_url
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
            {
                return Err(invalid_image("input_image must be a data URL or file_id"));
            }
            decode_inline_image(image_url).map(ImageJob::Upload)
        }
        (None, None) => Err(invalid_image("input_image must be a data URL or file_id")),
    }
}

fn decode_inline_image(data_url: &str) -> Result<InlineImage, AttachmentFailure> {
    let Some((_, rest)) = data_url.split_once(':') else {
        return Err(invalid_image("input_image data URL is invalid"));
    };
    let Some((metadata, encoded)) = rest.split_once(',') else {
        return Err(invalid_image("input_image data URL is invalid"));
    };
    let base64 = declared_image_base64(metadata)?;
    let decoded =
        path_unescape(encoded).map_err(|_| invalid_image("input_image data URL is invalid"))?;
    let image_bytes = if base64 {
        let text = std::str::from_utf8(&decoded)
            .map_err(|_| invalid_image("input_image data URL is invalid"))?;
        decode_base64(text)?
    } else {
        decoded
    };
    if image_bytes.is_empty() {
        return Err(invalid_image("input_image data URL is invalid"));
    }
    let (media_type, filename) = sniff_image(&image_bytes)
        .ok_or_else(|| invalid_image("input_image must be PNG, JPEG, GIF, or WebP"))?;
    Ok(InlineImage {
        media_type,
        filename,
        bytes: image_bytes,
    })
}

fn declared_image_base64(metadata: &str) -> Result<bool, AttachmentFailure> {
    let mut media = "";
    let mut base64 = false;
    for (index, part) in metadata.split(';').enumerate() {
        let part = part.trim();
        if index == 0 {
            media = part;
        } else if part.eq_ignore_ascii_case("base64") {
            base64 = true;
        }
    }
    if !media.to_ascii_lowercase().starts_with("image/") || !media.contains('/') {
        return Err(invalid_image("input_image data URL is invalid"));
    }
    Ok(base64)
}

fn sniff_image(image_bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if image_bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "image.png"))
    } else if image_bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "image.jpeg"))
    } else if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "image.gif"))
    } else if image_bytes.len() >= 12
        && image_bytes.starts_with(b"RIFF")
        && &image_bytes[8..12] == b"WEBP"
    {
        Some(("image/webp", "image.webp"))
    } else {
        None
    }
}

fn attachment_url(responses_url: &url::Url) -> Result<String, &'static str> {
    if responses_url.host_str().is_none()
        || !matches!(responses_url.scheme(), "https" | "http")
        || !responses_url.username().is_empty()
        || responses_url.password().is_some()
        || responses_url.query().is_some()
        || responses_url.fragment().is_some()
    {
        return Err("unsupported responses url");
    }
    let mut base = responses_url.clone();
    let path = base.path().trim_end_matches('/').to_string();
    base.set_path(&path);
    let joined = base
        .join("attachments")
        .map_err(|_| "unsupported responses url")?;
    if joined.scheme() != base.scheme()
        || joined.host_str() != base.host_str()
        || joined.port_or_known_default() != base.port_or_known_default()
    {
        return Err("unsupported responses url");
    }
    Ok(joined.to_string())
}

fn set_image_part(
    request_fields: &mut Value,
    input_index: usize,
    content_index: usize,
    file_id: &str,
) {
    if let Some(slot) = request_fields
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .and_then(|input_items| input_items.get_mut(input_index))
        .and_then(|input_item| input_item.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|content_parts| content_parts.get_mut(content_index))
    {
        *slot = serde_json::json!({
            "type": "input_image",
            "file_id": file_id,
        });
    }
}

async fn upload_image(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    endpoint: &str,
    headers: &HeaderMap,
    image: &InlineImage,
) -> Result<String, AttachmentFailure> {
    let (content_type, multipart_bytes) = multipart_body(image);
    let mut headers = headers.clone();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(|_| upload_failed())?,
    );
    let upload_request = runtime
        .request_client(candidate_id)
        .post(endpoint)
        .headers(headers)
        .header(ACCEPT, "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .body(multipart_bytes);
    let upstream = runtime
        .send_authorized_request(
            candidate_id,
            upload_request,
            crate::runtime::AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: None,
                lease: None,
            },
        )
        .await
        .map_err(|error| match error {
            crate::runtime::AuthorizedRequestError::Transport(error) => {
                AttachmentFailure::Retry(AttemptFailure::transport(&error))
            }
            _ => upload_failed(),
        })?;
    let status = upstream.response.status();
    if !status.is_success() {
        let failure = AttemptFailure::status_with_body(status, None);
        let no_fallback = matches!(
            status,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::PROXY_AUTHENTICATION_REQUIRED
                | StatusCode::TOO_MANY_REQUESTS
        );
        return Err(if no_fallback {
            AttachmentFailure::Reject(failure)
        } else {
            AttachmentFailure::Retry(failure)
        });
    }
    let upload_response_body = read_capped(upstream.response, MAX_UPLOAD_RESPONSE_BYTES).await?;
    let upload_response: Value =
        serde_json::from_slice(&upload_response_body).map_err(|_| upload_failed())?;
    upload_response
        .get("openai_file_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|file_id| !file_id.is_empty() && file_id.len() <= MAX_FILE_ID_LEN)
        .map(str::to_string)
        .ok_or_else(upload_failed)
}

async fn read_capped(
    mut upstream_response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, AttachmentFailure> {
    let mut response_body = Vec::new();
    loop {
        match upstream_response.chunk().await {
            Ok(Some(chunk)) => {
                if response_body.len().saturating_add(chunk.len()) > limit {
                    return Err(upload_failed());
                }
                response_body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(response_body),
            Err(_) => return Err(upload_failed()),
        }
    }
}

fn multipart_body(image: &InlineImage) -> (String, Vec<u8>) {
    let boundary = multipart_boundary(&image.bytes);
    let header = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
        image.filename, image.media_type
    );
    let mut multipart_body =
        Vec::with_capacity(header.len() + image.bytes.len() + boundary.len() + 8);
    multipart_body.extend_from_slice(header.as_bytes());
    multipart_body.extend_from_slice(&image.bytes);
    multipart_body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        multipart_body,
    )
}

fn multipart_boundary(image_bytes: &[u8]) -> String {
    let mut salt = 0_u32;
    loop {
        let mut digest = Sha256::new();
        digest.update(image_bytes);
        digest.update(salt.to_le_bytes());
        let boundary = format!("zenithbp{}", &hex::encode(digest.finalize())[..16]);
        if !contains_slice(image_bytes, boundary.as_bytes()) {
            return boundary;
        }
        salt = salt.wrapping_add(1);
    }
}

fn attachment_key(
    endpoint: &str,
    candidate_id: &str,
    credential_fingerprint: &[u8; 32],
    media_type: &str,
    image_bytes: &[u8],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(endpoint.as_bytes());
    digest.update([0]);
    digest.update(candidate_id.as_bytes());
    digest.update([0]);
    digest.update(credential_fingerprint);
    digest.update([0]);
    digest.update(media_type.as_bytes());
    digest.update([0]);
    digest.update(image_bytes);
    let digest = digest.finalize();
    let mut key = [0_u8; 32];
    key.copy_from_slice(&digest);
    key
}

fn attachment_cache() -> &'static Mutex<AttachmentCache> {
    static CACHE: OnceLock<Mutex<AttachmentCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(AttachmentCache {
            order: VecDeque::new(),
            entries: HashMap::new(),
        })
    })
}

fn cached_file_id(key: &[u8; 32]) -> Option<String> {
    let mut cache = crate::poison::mutex(attachment_cache());
    let file_id = cache.entries.get(key)?.clone();
    if let Some(position) = cache.order.iter().position(|cache_key| cache_key == key) {
        if let Some(cache_key) = cache.order.remove(position) {
            cache.order.push_front(cache_key);
        }
    }
    Some(file_id)
}

fn remember_file_id(key: [u8; 32], file_id: String) {
    let mut cache = crate::poison::mutex(attachment_cache());
    if let std::collections::hash_map::Entry::Occupied(mut entry) = cache.entries.entry(key) {
        entry.insert(file_id);
        if let Some(position) = cache.order.iter().position(|cache_key| *cache_key == key) {
            if let Some(cache_key) = cache.order.remove(position) {
                cache.order.push_front(cache_key);
            }
        }
        return;
    }
    cache.entries.insert(key, file_id);
    cache.order.push_front(key);
    while cache.order.len() > MAX_ATTACHMENT_CACHE {
        if let Some(oldest) = cache.order.pop_back() {
            cache.entries.remove(&oldest);
        }
    }
}

fn invalid_image(message: &'static str) -> AttachmentFailure {
    AttachmentFailure::Reject(AttemptFailure::rejected_request(message))
}

fn upload_failed() -> AttachmentFailure {
    AttachmentFailure::Retry(AttemptFailure::status_with_body(
        StatusCode::BAD_GATEWAY,
        None,
    ))
}

fn path_unescape(escaped_path: &str) -> Result<Vec<u8>, ()> {
    let bytes = escaped_path.as_bytes();
    let mut decoded_bytes = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded_bytes.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(());
        }
        let high = hex_value(bytes[index + 1]).ok_or(())?;
        let low = hex_value(bytes[index + 2]).ok_or(())?;
        decoded_bytes.push((high << 4) | low);
        index += 3;
    }
    Ok(decoded_bytes)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_base64(base64_text: &str) -> Result<Vec<u8>, AttachmentFailure> {
    let compact;
    let encoded = if base64_text.as_bytes().iter().any(u8::is_ascii_whitespace) {
        compact = base64_text
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>();
        compact.as_str()
    } else {
        base64_text
    };
    STANDARD
        .decode(encoded)
        .map_err(|_| invalid_image("input_image data URL is invalid"))
}

fn contains_slice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests;
