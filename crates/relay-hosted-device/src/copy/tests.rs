use super::*;
use serde_json::json;
use std::{
    cell::Cell,
    pin::pin,
    task::{Context, Poll, Waker},
};

const PROVIDER: &str = "fixture-api-secret";
const PROXY: &str = "http://fixture-user:fixture-secret@127.0.0.1:8090/";

fn scope() -> (Destination, CopyPlan) {
    let installation_id = Uuid::from_u128(1);
    let device_id = Uuid::from_u128(2);
    let origin = "https://copy.example";
    let target = Destination::parse(&format!(
        "{origin}/admin/connections#relay-device={installation_id}.{device_id}"
    ))
    .unwrap();
    let plan = CopyPlan {
        origin: origin.to_owned(),
        installation_id,
        device_id,
        operation_id: Uuid::from_u128(3),
        source_id: Uuid::from_u128(4),
        label: "Fixture source".to_owned(),
        endpoint: "https://source.example/v1".to_owned(),
        proxy: CopyProxy::from_route(PROXY),
    };
    (target, plan)
}

#[test]
fn preview_and_record_contain_no_provider_or_proxy_secret() {
    let (target, plan) = scope();
    let key = CopyKey::create(&target, plan.clone(), PROVIDER, Some(PROXY)).unwrap();
    let record = key.protected_record();
    let k = CopyKey::restore(&record).unwrap();
    let body = serde_json::to_string(k.review()).unwrap();
    for forbidden in [
        PROVIDER,
        "fixture-user",
        "fixture-secret",
        key.proof.bearer().as_str(),
    ] {
        assert!(!body.contains(forbidden));
    }
    assert!(!record.contains(PROVIDER));
    assert!(!record.contains(PROXY));
    assert!(body.contains("http://127.0.0.1:8090"));
    let code = k.review().code();
    assert!(CopyReview::from_code(&code).is_some());
    assert!(CopyProof::presented(&code).is_none());
    let commitment = k.review().commitment_bytes().unwrap();
    assert!(
        k.proof
            .matches_payload(&plan, &commitment, PROVIDER, Some(PROXY))
    );
    assert!(!k.proof.matches_payload(&plan, &commitment, PROVIDER, None));
    assert!(
        !k.proof
            .matches_payload(&plan, &commitment, "different-key", Some(PROXY))
    );
    assert!(!k.proof.matches_payload(
        &plan,
        &commitment,
        PROVIDER,
        Some("http://fixture-user:changed@127.0.0.1:8090/")
    ));
    let mut edited = plan.clone();
    edited.endpoint = "https://other-source.example/v1".to_owned();
    assert!(
        !k.proof
            .matches_payload(&edited, &commitment, PROVIDER, Some(PROXY))
    );
}

#[test]
fn preview_rejects_downgrades_ambiguous_urls_and_partial_proxy_auth() {
    let (_, plan) = scope();
    assert!(plan.valid());
    for endpoint in [
        "https://user:secret@source.example/v1",
        "https://source.example/v1?key=secret",
        "https://source.example/v1#secret",
        "https://source.example/v1/embedded-key",
        "http://source.example/v1",
        "https://copy.example/v1",
    ] {
        assert!(
            !CopyPlan {
                endpoint: endpoint.to_owned(),
                ..plan.clone()
            }
            .valid()
        );
    }
    assert!(
        !CopyPlan {
            origin: "http://localhost:1443".to_owned(),
            ..plan
        }
        .valid()
    );
    for proxy in [
        "http://user@127.0.0.1:8090/",
        "http://:password@127.0.0.1:8090/",
        "http://127.0.0.1:8090/path",
        "http://127.0.0.1/",
    ] {
        assert!(CopyProxy::from_route(proxy).is_none());
    }
    assert!(CopyProxy::from_route("http://127.0.0.1:8090/").is_some());
}

#[test]
fn approval_and_receipt_have_to_belong_to_the_reviewed_operation() {
    let (_, plan) = scope();
    let good = json!({"data": {"installation_id":plan.installation_id,"device_id":plan.device_id,
        "operation_id":plan.operation_id,"state":"approved"},
        "meta":{"api_version":"1","request_id":plan.source_id}});
    let parse = |v: &serde_json::Value| parse_reply(&plan, &serde_json::to_vec(v).unwrap());
    assert_eq!(parse(&good), Ok(CopyState::Approved));
    for key in ["installation_id", "device_id", "operation_id", "state"] {
        let mut wrong = good.clone();
        wrong["data"][key] = json!("8d78cd24-e53a-42f8-97da-031bf4e1187c");
        assert_eq!(parse(&wrong), Err(RESPONSE));
    }
}

fn ready<T>(f: impl Future<Output = T>) -> T {
    match pin!(f).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("fixture work should be ready"),
    }
}

#[test]
fn commit_reads_only_after_approval_and_sends_only_the_reviewed_snapshot() {
    let (target, plan) = scope();
    let key = CopyKey::create(&target, plan.clone(), PROVIDER, Some(PROXY)).unwrap();
    let no_input = async || panic!("read a source without a matching approval");
    let no_send = async |_| panic!("sent an unapproved or changed source");
    for result in [
        Ok(CopyState::AwaitingApproval),
        Ok(CopyState::Expired),
        Ok(CopyState::Committed),
        Err(RESPONSE),
    ] {
        assert_eq!(
            ready(key.commit_after(async { result }, no_input, no_send)),
            result
        );
    }

    let passed = Cell::new(false);
    let changed_origin = CopyPlan {
        origin: "https://other.example".to_owned(),
        ..plan.clone()
    };
    for (plan, provider, proxy) in [
        (changed_origin, PROVIDER, PROXY),
        (plan.clone(), "replacement-key", PROXY),
        (
            plan.clone(),
            PROVIDER,
            "http://fixture-user:changed@127.0.0.1:8090/",
        ),
    ] {
        passed.set(false);
        let result = ready(key.commit_after(
            async {
                passed.set(true);
                Ok(CopyState::Approved)
            },
            async || {
                assert!(passed.get());
                Ok(CopyInput {
                    plan,
                    provider: Zeroizing::new(provider.to_owned()),
                    proxy: Some(Zeroizing::new(proxy.to_owned())),
                })
            },
            no_send,
        ));
        assert_eq!(result, Err("copy_source_changed"));
    }

    let applied = ready(key.commit_after(
        async { Ok(CopyState::Approved) },
        async || {
            Ok(CopyInput {
                plan,
                provider: Zeroizing::new(PROVIDER.to_owned()),
                proxy: Some(Zeroizing::new(PROXY.to_owned())),
            })
        },
        async |body| {
            assert!(body == format!("{PROVIDER}\n{PROXY}").as_bytes());
            Ok(CopyState::Committed)
        },
    ));
    assert_eq!(applied, Ok(CopyState::Committed));
}
