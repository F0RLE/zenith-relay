use super::{
    AccountModelsClient, ModelDiscoveryFailure, ModelDiscoveryFailureCode,
    MAX_BASIS_POINTS_ACCESS_BYTES, MAX_MODELS_RESPONSE_BYTES,
};
use crate::{
    scheduler::refresh::http::{HttpClass, HttpSendError},
    transport::collect_limited,
    Error,
};
use std::time::{Duration, SystemTime};

const TRANSPORT_RETRY_DELAY: Duration = Duration::from_millis(250);

impl AccountModelsClient {
    /// Only the native catalog GET is replayable here, never generation or a
    /// BPS permission check. The owning refresh job remains single-flight and
    /// every physical send re-enters the shared HTTP gate and revision fence.
    pub(super) async fn read_response(
        &self,
        request: reqwest::RequestBuilder,
        basis_points: bool,
    ) -> Result<(reqwest::StatusCode, Option<u64>, Vec<u8>), ModelDiscoveryFailure> {
        let mut retry_transport = !basis_points;
        loop {
            let attempt = request.try_clone().ok_or_else(|| {
                ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::InvalidEndpoint)
            })?;
            let (response, permit) = match self
                .scope
                .send(&self.http, attempt, HttpClass::Ordinary)
                .await
            {
                Ok(sent) => sent,
                Err(HttpSendError::Transport { .. }) if retry_transport => {
                    retry_transport = false;
                    tokio::time::sleep(TRANSPORT_RETRY_DELAY).await;
                    continue;
                }
                Err(error) => {
                    return Err(ModelDiscoveryFailure::retryable(if error.is_timeout() {
                        ModelDiscoveryFailureCode::Timeout
                    } else {
                        ModelDiscoveryFailureCode::Transport
                    }));
                }
            };
            let status = response.status();
            let retry_after_ms =
                crate::transport::retry_after_ms(response.headers(), SystemTime::now());
            let body = collect_limited(
                response,
                if basis_points {
                    MAX_BASIS_POINTS_ACCESS_BYTES
                } else {
                    MAX_MODELS_RESPONSE_BYTES
                },
            )
            .await;
            // Release scarce HTTP capacity before waiting or starting another
            // attempt, including when a response body times out mid-transfer.
            drop(permit);
            match body {
                Ok(body) => return Ok((status, retry_after_ms, body)),
                Err(Error::Upstream(_))
                    if retry_transport && status.is_success() && retry_after_ms.is_none() =>
                {
                    retry_transport = false;
                    tokio::time::sleep(TRANSPORT_RETRY_DELAY).await;
                }
                Err(error) => {
                    let mut failure = match error {
                        Error::UpstreamBodyTooLarge => {
                            ModelDiscoveryFailure::new(ModelDiscoveryFailureCode::ResponseTooLarge)
                        }
                        Error::Upstream(error) if error.is_timeout() => {
                            ModelDiscoveryFailure::retryable(ModelDiscoveryFailureCode::Timeout)
                        }
                        _ => ModelDiscoveryFailure::retryable(ModelDiscoveryFailureCode::Transport),
                    };
                    failure.retry_after_ms = retry_after_ms;
                    return Err(failure);
                }
            }
        }
    }
}
