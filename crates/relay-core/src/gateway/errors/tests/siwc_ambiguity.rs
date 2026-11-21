use super::*;

#[test]
fn siwc_quota_discriminators_do_not_fall_through_explicit_fields() {
    let quota = "subscription_sharing_usage_limit_exceeded";
    for code in [
        Value::Null,
        json!(42),
        json!([]),
        json!({}),
        json!(""),
        json!("provider_unknown"),
    ] {
        for body in [
            json!({"error":{"code":code,"type":quota}}),
            json!({"error":{"code":code,"upstream_error":{"code":quota}}}),
            json!({"error":{"code":code},"response":{"error":{"code":quota}}}),
            json!({"error":{"code":code},"code":quota}),
        ] {
            assert_no_quota_proof(&body.to_string());
        }
    }
    for body in [
        json!({"error":null,"code":quota}),
        json!({"error":[],"response":{"error":{"code":quota}}}),
        json!({"error":{"type":"provider_unknown","upstream_error":{"code":quota}}}),
    ] {
        assert_no_quota_proof(&body.to_string());
    }
}

#[test]
fn siwc_duplicate_json_fields_do_not_authorize_http_or_sse_replay() {
    for body in [
        r#"{"error":{"code":"provider_unknown","code":"subscription_sharing_usage_limit_exceeded"}}"#,
        r#"{"error":{"code":"provider_unknown","co\u0064e":"subscription_sharing_usage_limit_exceeded"}}"#,
        r#"{"error":{"code":"provider_unknown"},"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#,
        r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"},"metadata":{"x":1,"x":2}}"#,
        r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"},"type":"response.completed","type":"error"}"#,
        r#"{"error":{"code":"provider_unknown","code":"rate_limit_exceeded"}}"#,
    ] {
        assert_no_quota_proof(body);
    }
}

fn assert_no_quota_proof(body: &str) {
    let failure =
        AttemptFailure::status_with_body(StatusCode::TOO_MANY_REQUESTS, Some(body.as_bytes()));
    assert!(!failure.cooldown_hint.siwc_usage_limit, "HTTP: {body}");
    assert!(
        !failure.cooldown_hint.explicit_transient_rejection,
        "HTTP: {body}"
    );
    let frame = format!("event: error\ndata: {body}\n\n");
    let event = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
    assert!(event.valid, "SSE fixture: {body}");
    assert!(!event.cooldown_hint.siwc_usage_limit, "SSE: {body}");
    assert!(
        !event.cooldown_hint.explicit_transient_rejection,
        "SSE: {body}"
    );
}

#[test]
fn transient_wait_permission_requires_the_primary_structured_discriminator() {
    for code in ["rate_limit_exceeded", "slow_down", "server_is_overloaded"] {
        for body in [
            json!({"error":{"code":code}}),
            json!({"response":{"error":{"code":code}}}),
            json!({"body":{"error":{"code":code}}}),
            json!({"code":code}),
        ] {
            let failure = AttemptFailure::status_with_body(
                StatusCode::TOO_MANY_REQUESTS,
                Some(body.to_string().as_bytes()),
            );
            assert!(failure.cooldown_hint.explicit_transient_rejection);
        }
        for body in [
            json!({"error":{"code":null,"type":code}}),
            json!({"error":{"code":"provider_unknown","type":code}}),
            json!({"error":{"code":"provider_unknown"},"code":code}),
            json!({"error":null,"response":{"error":{"code":code}}}),
        ] {
            assert_no_quota_proof(&body.to_string());
        }
    }
}
