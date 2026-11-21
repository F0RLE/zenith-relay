use crate::protocol::images::{ImageEnvelope, MAX_IMAGE_BODY_BYTES};
use crate::scheduler::rotation::{ExecutionObservation, SharedRequestBudget};
use reqwest::ResponseBuilderExt;

/// Inspect a complete, bounded image refusal before renewing and resending it.
pub(super) async fn inspect(
    mut response: reqwest::Response,
    budget: &SharedRequestBudget,
) -> (reqwest::Response, bool) {
    let status = response.status();
    let version = response.version();
    let url = response.url().clone();
    let mut headers = response.headers().clone();
    let extensions = std::mem::take(response.extensions_mut());
    let collected =
        crate::transport::collect_with_progress_limit(response, None, MAX_IMAGE_BODY_BYTES, |_| {
            false
        })
        .await;
    let complete = collected.is_ok();
    let bytes = collected.unwrap_or_else(|partial| partial.bytes);
    let replay_safe = complete
        && serde_json::from_slice::<ImageEnvelope<'_>>(&bytes)
            .is_ok_and(|envelope| envelope.error.is_some() && !envelope.has_generation_evidence());
    if !replay_safe {
        budget.with_budget(|request| request.observe_execution(ExecutionObservation::unknown()));
    }
    if !complete {
        headers.remove(reqwest::header::CONTENT_LENGTH);
    }
    let mut restored = axum::http::Response::builder()
        .status(status)
        .version(version)
        .url(url)
        .body(reqwest::Body::from(bytes))
        .expect("existing response status and version are valid");
    *restored.headers_mut() = headers;
    restored.extensions_mut().extend(extensions);
    (reqwest::Response::from(restored), replay_safe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct SyntheticObservation;

    #[tokio::test]
    async fn inspection_preserves_response_metadata_and_closes_unsafe_replay() {
        for (body, expected) in [
            (r#"{"error":{"code":"token_expired"}}"#, true),
            (
                r#"{"error":{"code":"token_expired"},"data":[{"b64_json":"YQ=="}]}"#,
                false,
            ),
            (
                r#"{"error":{"code":"token_expired"},"svg_usage":{"output_tokens":1}}"#,
                false,
            ),
            (
                r#"{"error":{"code":"token_expired"},"usage":{"output_tokens_details":{"reasoning_tokens":1}}}"#,
                false,
            ),
            (r#"{"error":{"code":"token_expired"},"data":42}"#, false),
            ("{}", false),
            ("invalid JSON", false),
        ] {
            let url = reqwest::Url::parse("https://example.invalid/v1/images/generations").unwrap();
            let response = axum::http::Response::builder()
                .status(reqwest::StatusCode::UNAUTHORIZED)
                .version(reqwest::Version::HTTP_2)
                .header("x-synthetic", "preserved")
                .url(url.clone())
                .extension(SyntheticObservation)
                .body(reqwest::Body::from(body))
                .unwrap();
            let budget = SharedRequestBudget::for_incoming_request(3);
            let (response, replay_safe) = inspect(reqwest::Response::from(response), &budget).await;
            assert_eq!(replay_safe, expected, "{body}");
            assert_eq!(budget.can_dispatch(), expected, "{body}");
            assert_eq!(response.url(), &url);
            assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
            assert_eq!(response.version(), reqwest::Version::HTTP_2);
            assert_eq!(response.headers()["x-synthetic"], "preserved");
            assert_eq!(
                response.extensions().get::<SyntheticObservation>(),
                Some(&SyntheticObservation)
            );
            assert_eq!(response.text().await.unwrap(), body);
        }
    }

    #[tokio::test]
    async fn interrupted_image_refusal_keeps_partial_bytes_but_never_replays() {
        let body = r#"{"error":{"code":"token_expired"}}"#;
        let stream = futures_util::stream::iter([
            Ok(bytes::Bytes::from_static(body.as_bytes())),
            Err(std::io::Error::other("synthetic interruption")),
        ]);
        let response = axum::http::Response::builder()
            .status(reqwest::StatusCode::UNAUTHORIZED)
            .header(reqwest::header::CONTENT_LENGTH, 1_000)
            .body(reqwest::Body::wrap_stream(stream))
            .unwrap();
        let budget = SharedRequestBudget::for_incoming_request(3);
        let (response, replay_safe) = inspect(reqwest::Response::from(response), &budget).await;
        assert!(!replay_safe);
        assert!(!budget.can_dispatch());
        assert!(
            !response
                .headers()
                .contains_key(reqwest::header::CONTENT_LENGTH)
        );
        assert_eq!(response.text().await.unwrap(), body);
    }
}
