use crate::runtime::AuthorizedRequestError;
use axum::body::HttpBody;
use futures_util::StreamExt;
use reqwest::{Client, Request, Response, Version};
use std::time::Duration;

/// Reqwest 0.13.5's H3 connector does not apply ClientBuilder::connect_timeout.
/// Its request body is polled only after QUIC establishment and request headers.
/// Bound that startup phase, not the provider's response or quiet reasoning.
pub(in crate::runtime) async fn execute(
    client: &Client,
    request: Request,
) -> Result<Response, AuthorizedRequestError> {
    execute_with_start_timeout(client, request, super::super::support::CONNECT_TIMEOUT).await
}

pub(super) async fn execute_with_start_timeout(
    client: &Client,
    mut request: Request,
    timeout: Duration,
) -> Result<Response, AuthorizedRequestError> {
    if request.version() != Version::HTTP_3 {
        return client
            .execute(request)
            .await
            .map_err(AuthorizedRequestError::Transport);
    }
    let (started, received) = tokio::sync::oneshot::channel();
    let body = request.body_mut().take().unwrap_or_default();
    if let Some(size) = body.size_hint().exact() {
        // wrap_stream has no exact byte size. Preserve the original wire length
        // without serializing or copying the payload again.
        request
            .headers_mut()
            .entry(reqwest::header::CONTENT_LENGTH)
            .or_insert_with(|| size.into());
    }
    let mut body = axum::body::Body::new(body).into_data_stream();
    let mut started = Some(started);
    *request.body_mut() = Some(reqwest::Body::wrap_stream(futures_util::stream::poll_fn(
        move |cx| {
            if let Some(started) = started.take() {
                let _ = started.send(());
            }
            body.poll_next_unpin(cx)
        },
    )));
    let send = client.execute(request);
    tokio::pin!(send);
    tokio::select! {
        biased;
        response = &mut send => response.map_err(AuthorizedRequestError::Transport),
        Ok(()) = received => send.await.map_err(AuthorizedRequestError::Transport),
        _ = tokio::time::sleep(timeout) => Err(AuthorizedRequestError::SendStartTimeout),
    }
}
