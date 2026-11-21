use super::*;
use std::{collections::VecDeque, sync::Mutex};

struct State {
    replies: VecDeque<Result<StatusCode, &'static str>>,
    balance_statuses: VecDeque<StatusCode>,
    requests: Vec<String>,
    recoveries: usize,
    marker: bool,
    current: bool,
    invalidate_on_reply: bool,
    record_fails: bool,
    allowed: VecDeque<bool>,
}

struct Host(Mutex<State>);

impl Host {
    fn new(replies: &[Result<StatusCode, &'static str>]) -> Self {
        Self(Mutex::new(State {
            replies: replies.iter().copied().collect(),
            balance_statuses: VecDeque::new(),
            requests: Vec::new(),
            recoveries: 0,
            marker: false,
            current: true,
            invalidate_on_reply: false,
            record_fails: false,
            allowed: VecDeque::new(),
        }))
    }
}

impl ResetHost for Host {
    type Authorization = usize;
    type Error = &'static str;

    fn validate(&self) -> Result<(), Self::Error> {
        if self.0.lock().unwrap().current {
            Ok(())
        } else {
            Err("stale")
        }
    }

    fn error(error: ResetCreditsError) -> Self::Error {
        error.message
    }

    async fn prepare(&self) -> Result<usize, Self::Error> {
        Ok(0)
    }

    async fn recover(&self, rejected: &usize) -> Result<usize, Self::Error> {
        self.0.lock().unwrap().recoveries += 1;
        Ok(rejected + 1)
    }

    async fn balance(&self, _: &usize) -> Result<ResetHttpResponse, Self::Error> {
        Ok(ResetHttpResponse {
            status: self
                .0
                .lock()
                .unwrap()
                .balance_statuses
                .pop_front()
                .unwrap_or(StatusCode::OK),
            response_body: br#"{"availableCount":1}"#.to_vec(),
        })
    }

    async fn record_dispatch(&self, _: Option<u64>) -> Result<bool, Self::Error> {
        self.validate()?;
        let mut state = self.0.lock().unwrap();
        if state.record_fails {
            return Err("persist");
        }
        if !state.allowed.pop_front().unwrap_or(true) {
            return Ok(false);
        }
        state.marker = true;
        Ok(true)
    }

    fn clear_rejected(&self) -> Result<(), Self::Error> {
        self.validate()?;
        self.0.lock().unwrap().marker = false;
        Ok(())
    }

    async fn redeem(&self, _: &usize, request_id: &str) -> Result<ResetHttpResponse, Self::Error> {
        let mut state = self.0.lock().unwrap();
        assert!(state.marker, "dispatch must follow durable persistence");
        state.requests.push(request_id.to_owned());
        if state.invalidate_on_reply {
            state.current = false;
        }
        Ok(ResetHttpResponse {
            status: state.replies.pop_front().expect("unexpected replay")?,
            response_body: Vec::new(),
        })
    }
}

#[tokio::test]
async fn authorization_retry_reuses_dispatch_identity() {
    let host = Host::new(&[Ok(StatusCode::UNAUTHORIZED), Ok(StatusCode::OK)]);
    assert!(redeem(&host, "synthetic-redemption").await.unwrap());
    let state = host.0.lock().unwrap();
    assert_eq!(
        state.requests,
        ["synthetic-redemption", "synthetic-redemption"]
    );
    assert_eq!(state.recoveries, 1);
    assert!(state.marker);
}

#[tokio::test]
async fn ambiguous_response_or_transport_failure_is_never_replayed() {
    for reply in [
        Err("transport"),
        Ok(StatusCode::REQUEST_TIMEOUT),
        Ok(StatusCode::CONFLICT),
        Ok(StatusCode::TOO_EARLY),
        Ok(StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let host = Host::new(&[reply]);
        assert!(redeem(&host, "synthetic-redemption").await.is_err());
        let state = host.0.lock().unwrap();
        assert_eq!(state.requests.len(), 1);
        assert_eq!(state.recoveries, 0);
        assert!(state.marker);
    }
}

#[tokio::test]
async fn failed_durable_write_prevents_provider_dispatch() {
    let host = Host::new(&[]);
    host.0.lock().unwrap().record_fails = true;
    assert_eq!(redeem(&host, "synthetic-redemption").await, Err("persist"));
    assert!(host.0.lock().unwrap().requests.is_empty());
}

#[tokio::test]
async fn replacement_during_request_prevents_old_completion_and_marker_cleanup() {
    let host = Host::new(&[Ok(StatusCode::UNAUTHORIZED)]);
    host.0.lock().unwrap().invalidate_on_reply = true;
    assert_eq!(redeem(&host, "synthetic-redemption").await, Err("stale"));
    let state = host.0.lock().unwrap();
    assert_eq!(state.recoveries, 0);
    assert!(state.marker);
}

#[tokio::test]
async fn policy_change_after_recovery_prevents_second_dispatch() {
    let host = Host::new(&[Ok(StatusCode::UNAUTHORIZED)]);
    host.0.lock().unwrap().allowed.extend([true, false]);
    assert!(!redeem(&host, "synthetic-redemption").await.unwrap());
    let state = host.0.lock().unwrap();
    assert_eq!(state.requests.len(), 1);
    assert!(!state.marker);
}

#[tokio::test]
async fn second_authorization_rejection_is_terminal() {
    let host = Host::new(&[Ok(StatusCode::UNAUTHORIZED), Ok(StatusCode::UNAUTHORIZED)]);
    assert!(redeem(&host, "synthetic-redemption").await.is_err());
    let state = host.0.lock().unwrap();
    assert_eq!(state.recoveries, 1);
    assert_eq!(state.requests.len(), 2);
    assert!(!state.marker);
}

#[tokio::test]
async fn balance_recovery_is_bounded_and_precedes_redemption() {
    let host = Host::new(&[]);
    host.0
        .lock()
        .unwrap()
        .balance_statuses
        .extend([StatusCode::UNAUTHORIZED; 2]);
    assert!(redeem(&host, "synthetic-redemption").await.is_err());
    let state = host.0.lock().unwrap();
    assert_eq!(state.recoveries, 1);
    assert!(state.requests.is_empty());
    assert!(!state.marker);
}
