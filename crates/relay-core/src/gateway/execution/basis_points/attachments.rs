//! Upload user images before a Basis Points request.
//!
//! The upstream body accepts `input_image.file_id` only. A data URL is decoded,
//! checked by its bytes, and uploaded to the attachments endpoint beside the
//! responses URL. Remote URLs and extra fields such as `detail` are not forwarded.
//! Image bytes, file IDs and credentials stay out of errors and logs.

use super::super::super::errors::AttemptFailure;
use crate::GatewayRuntime;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::StatusCode;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
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
    data: Vec<u8>,
}

enum ImageJob {
    File(String),
    Upload(InlineImage),
}

struct LocatedJob {
    item: usize,
    part: usize,
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
    body: Vec<u8>,
) -> Result<Vec<u8>, AttachmentFailure> {
    if !body.windows(13).any(|window| window == b"\"input_image\"") {
        return Ok(body);
    }
    let mut request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return Ok(body),
    };
    let jobs = stage_input_images(&request)?;
    if jobs.is_empty() {
        return Ok(body);
    }
    let endpoint = attachment_url(responses_url).map_err(|_| upload_failed())?;
    for job in &jobs {
        if let ImageJob::File(file_id) = &job.job {
            set_image_part(&mut request, job.item, job.part, file_id);
        }
    }
    for job in jobs {
        let ImageJob::Upload(image) = job.job else {
            continue;
        };
        let key = attachment_key(&endpoint, candidate_id, image.media_type, &image.data);
        let file_id = if let Some(file_id) = cached_file_id(&key) {
            file_id
        } else {
            let file_id = upload_image(runtime, candidate_id, &endpoint, headers, &image).await?;
            remember_file_id(key, file_id.clone());
            file_id
        };
        set_image_part(&mut request, job.item, job.part, &file_id);
    }
    serde_json::to_vec(&request).map_err(|_| upload_failed())
}

fn stage_input_images(body: &Value) -> Result<Vec<LocatedJob>, AttachmentFailure> {
    let Some(items) = body.get("input").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut jobs = Vec::new();
    for (item_index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            continue;
        };
        let role = object.get("role").and_then(Value::as_str).unwrap_or("");
        let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
        if role != "user" || !(kind.is_empty() || kind == "message") {
            continue;
        }
        let Some(parts) = object.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (part_index, part) in parts.iter().enumerate() {
            let Some(part) = part.as_object() else {
                continue;
            };
            if part.get("type").and_then(Value::as_str) != Some("input_image") {
                continue;
            }
            jobs.push(LocatedJob {
                item: item_index,
                part: part_index,
                job: image_job(part)?,
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
            if image_url.len() < 5 || !image_url[..5].eq_ignore_ascii_case("data:") {
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
    let data = if base64 {
        let text = std::str::from_utf8(&decoded)
            .map_err(|_| invalid_image("input_image data URL is invalid"))?;
        decode_base64(text)?
    } else {
        decoded
    };
    if data.is_empty() || data.len() > MAX_IMAGE_BYTES {
        return Err(invalid_image("input_image data URL is invalid"));
    }
    let (media_type, filename) = sniff_image(&data)
        .ok_or_else(|| invalid_image("input_image must be PNG, JPEG, GIF, or WebP"))?;
    Ok(InlineImage {
        media_type,
        filename,
        data,
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

fn sniff_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "image.png"))
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "image.jpeg"))
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(("image/gif", "image.gif"))
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
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

fn set_image_part(body: &mut Value, item: usize, part: usize, file_id: &str) {
    if let Some(slot) = body
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .and_then(|items| items.get_mut(item))
        .and_then(|item| item.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|parts| parts.get_mut(part))
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
    let (content_type, payload) = multipart_body(image);
    let mut headers = headers.clone();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&content_type).map_err(|_| upload_failed())?,
    );
    let request = runtime
        .request_client(candidate_id)
        .post(endpoint)
        .headers(headers)
        .body(payload);
    let upstream = runtime
        .send_authorized_request(candidate_id, request, None, None, None, None)
        .await
        .map_err(|error| match error {
            crate::runtime::AuthorizedRequestError::Transport(error) => {
                AttachmentFailure::Retry(AttemptFailure::transport(&error))
            }
            _ => upload_failed(),
        })?;
    let status = upstream.response.status();
    if !status.is_success() {
        return Err(AttachmentFailure::Retry(AttemptFailure::status_with_body(
            status, None,
        )));
    }
    let body = read_capped(upstream.response, MAX_UPLOAD_RESPONSE_BYTES).await?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| upload_failed())?;
    value
        .get("openai_file_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|file_id| !file_id.is_empty() && file_id.len() <= MAX_FILE_ID_LEN)
        .map(str::to_string)
        .ok_or_else(upload_failed)
}

async fn read_capped(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, AttachmentFailure> {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > limit {
                    return Err(upload_failed());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(_) => return Err(upload_failed()),
        }
    }
}

fn multipart_body(image: &InlineImage) -> (String, Vec<u8>) {
    let boundary = multipart_boundary(&image.data);
    let header = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
        image.filename, image.media_type
    );
    let mut body = Vec::with_capacity(header.len() + image.data.len() + boundary.len() + 8);
    body.extend_from_slice(header.as_bytes());
    body.extend_from_slice(&image.data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn multipart_boundary(data: &[u8]) -> String {
    let mut salt = 0_u32;
    loop {
        let mut digest = Sha256::new();
        digest.update(data);
        digest.update(salt.to_le_bytes());
        let boundary = format!("zenithbp{}", &hex::encode(digest.finalize())[..16]);
        if !contains_slice(data, boundary.as_bytes()) {
            return boundary;
        }
        salt = salt.wrapping_add(1);
    }
}

fn attachment_key(endpoint: &str, candidate_id: &str, media_type: &str, data: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(endpoint.as_bytes());
    digest.update([0]);
    digest.update(candidate_id.as_bytes());
    digest.update([0]);
    digest.update(media_type.as_bytes());
    digest.update([0]);
    digest.update(data);
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
    if let Some(position) = cache.order.iter().position(|item| item == key) {
        if let Some(item) = cache.order.remove(position) {
            cache.order.push_front(item);
        }
    }
    Some(file_id)
}

fn remember_file_id(key: [u8; 32], file_id: String) {
    let mut cache = crate::poison::mutex(attachment_cache());
    if let std::collections::hash_map::Entry::Occupied(mut entry) = cache.entries.entry(key) {
        entry.insert(file_id);
        if let Some(position) = cache.order.iter().position(|item| *item == key) {
            if let Some(item) = cache.order.remove(position) {
                cache.order.push_front(item);
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

fn path_unescape(input: &str) -> Result<Vec<u8>, ()> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(());
        }
        let high = hex_value(bytes[index + 1]).ok_or(())?;
        let low = hex_value(bytes[index + 2]).ok_or(())?;
        output.push((high << 4) | low);
        index += 3;
    }
    Ok(output)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_base64(input: &str) -> Result<Vec<u8>, AttachmentFailure> {
    let compact;
    let encoded = if input.as_bytes().iter().any(u8::is_ascii_whitespace) {
        compact = input
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>();
        compact.as_str()
    } else {
        input
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
mod tests {
    use super::*;
    use serde_json::json;

    fn png_bytes() -> Vec<u8> {
        STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
            .unwrap()
    }

    fn data_url(bytes: &[u8]) -> String {
        format!("data:image/png;base64,{}", STANDARD.encode(bytes))
    }

    fn failure_message(result: Result<Vec<LocatedJob>, AttachmentFailure>) -> &'static str {
        match result {
            Err(AttachmentFailure::Reject(failure)) => failure.message,
            Err(AttachmentFailure::Retry(failure)) => panic!("{}", failure.message),
            Ok(_) => panic!("expected a request rejection"),
        }
    }

    fn expect_jobs(result: Result<Vec<LocatedJob>, AttachmentFailure>) -> Vec<LocatedJob> {
        result.unwrap_or_else(|failure| match failure {
            AttachmentFailure::Reject(failure) | AttachmentFailure::Retry(failure) => {
                panic!("{}", failure.message)
            }
        })
    }

    fn expect_image(result: Result<InlineImage, AttachmentFailure>) -> InlineImage {
        result.unwrap_or_else(|failure| match failure {
            AttachmentFailure::Reject(failure) | AttachmentFailure::Retry(failure) => {
                panic!("{}", failure.message)
            }
        })
    }

    #[test]
    fn user_data_url_is_staged_and_existing_file_id_drops_detail() {
        let png = png_bytes();
        let body = json!({
            "input": [
                {
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "look"},
                        {"type": "input_image", "image_url": data_url(&png), "detail": "high"}
                    ]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_image", "file_id": "file-existing", "detail": "low"}]
                },
                {
                    "role": "assistant",
                    "content": [{"type": "input_image", "image_url": "https://example.com/a.png", "detail": "auto"}]
                }
            ]
        });
        let jobs = expect_jobs(stage_input_images(&body));
        assert!(matches!(jobs[0].job, ImageJob::Upload(_)));
        assert!(matches!(&jobs[1].job, ImageJob::File(file_id) if file_id == "file-existing"));
        assert_eq!(jobs.len(), 2);
        let mut rewritten = body;
        set_image_part(&mut rewritten, 1, 0, "file-existing");
        assert_eq!(
            rewritten["input"][1]["content"][0],
            json!({"type": "input_image", "file_id": "file-existing"})
        );
        assert_eq!(
            rewritten["input"][2]["content"][0]["image_url"],
            "https://example.com/a.png"
        );
    }

    #[test]
    fn image_url_object_and_supported_signatures_decode() {
        let png = png_bytes();
        let body = json!({
            "input": [{
                "role": "user",
                "content": [{"type": "input_image", "image_url": {"url": data_url(&png), "detail": "auto"}, "detail": "low"}]
            }]
        });
        let jobs = expect_jobs(stage_input_images(&body));
        let ImageJob::Upload(image) = &jobs[0].job else {
            panic!("upload");
        };
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.filename, "image.png");
        assert_eq!(image.data, png);

        let jpeg = expect_image(decode_inline_image("data:image/jpg;base64,/9j/AA=="));
        assert_eq!(jpeg.filename, "image.jpeg");
        let gif = expect_image(decode_inline_image("data:image/gif;base64,R0lGODlh"));
        assert_eq!(gif.filename, "image.gif");
        let webp = expect_image(decode_inline_image(
            "data:image/webp;base64,UklGRgAAAABXRUJQ",
        ));
        assert_eq!(webp.media_type, "image/webp");
    }

    #[test]
    fn conflicting_remote_and_unknown_images_are_rejected() {
        let both = json!({
            "input": [{"role": "user", "content": [{
                "type": "input_image",
                "file_id": "file-1",
                "image_url": "data:image/png;base64,AA=="
            }]}]
        });
        assert_eq!(
            failure_message(stage_input_images(&both)),
            "input_image cannot contain both image_url and file_id"
        );
        let remote = json!({
            "input": [{"role": "user", "content": [{
                "type": "input_image",
                "image_url": "https://example.com/a.png"
            }]}]
        });
        assert_eq!(
            failure_message(stage_input_images(&remote)),
            "input_image must be a data URL or file_id"
        );
        let text = json!({
            "input": [{"role": "user", "content": [{
                "type": "input_image",
                "image_url": "data:image/png;base64,aGVsbG8="
            }]}]
        });
        assert_eq!(
            failure_message(stage_input_images(&text)),
            "input_image must be PNG, JPEG, GIF, or WebP"
        );
    }

    #[test]
    fn attachment_url_replaces_the_responses_segment() {
        let responses =
            url::Url::parse("https://bps.openai.com/basispoints/api/responses").unwrap();
        assert_eq!(
            attachment_url(&responses).unwrap(),
            "https://bps.openai.com/basispoints/api/attachments"
        );
        let credentials =
            url::Url::parse("https://user:secret@bps.openai.com/basispoints/api/responses")
                .unwrap();
        assert!(attachment_url(&credentials).is_err());
    }

    #[test]
    fn multipart_names_the_detected_file_and_contains_only_the_image() {
        let image = expect_image(decode_inline_image(&format!(
            "DATA:image/png;BASE64,{}",
            STANDARD.encode(png_bytes())
        )));
        let (content_type, body) = multipart_body(&image);
        let header = "multipart/form-data; boundary=";
        assert!(content_type.starts_with(header));
        let boundary = content_type.trim_start_matches(header);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("filename=\"image.png\""));
        assert!(text.contains("Content-Type: image/png"));
        assert!(text.contains(boundary));
        assert!(body
            .windows(png_bytes().len())
            .any(|window| window == png_bytes()));
        assert!(!text.contains("Bearer"));
    }

    #[test]
    fn attachment_cache_reuses_the_file_id_without_storing_bytes() {
        let key = attachment_key(
            "https://bps.openai.com/attachments",
            "account",
            "image/png",
            b"png-bytes",
        );
        remember_file_id(key, "file-cached".to_string());
        assert_eq!(cached_file_id(&key).as_deref(), Some("file-cached"));
    }
}
