use super::*;
use crate::UpstreamProtocolPolicy;
use serde_json::json;

#[test]
fn websocket_application_protocol_refreshes_each_request_but_not_in_flight_work() {
    let runtime = runtime();
    let handshake = runtime.authenticate_secret("local-secret").unwrap();
    let parse = |body: Value| {
        ClientRequest::parse(
            &runtime,
            &handshake,
            &HeaderMap::new(),
            &serde_json::to_vec(&body).unwrap(),
        )
    };
    let fresh = json!({"model":"relay/upstream-model","input":"Synthetic input"});
    let active = parse(fresh.clone()).unwrap_or_else(|error| panic!("{}", error.message));
    assert!(runtime.set_key_upstream_protocol("key", UpstreamProtocolPolicy::Responses));
    let next = parse(fresh.clone()).unwrap_or_else(|error| panic!("{}", error.message));
    assert_eq!(handshake.upstream_protocol, UpstreamProtocolPolicy::Auto);
    assert_eq!(active.upstream_protocol, UpstreamProtocolPolicy::Auto);
    assert_eq!(next.upstream_protocol, UpstreamProtocolPolicy::Responses);

    // An edit after parsing cannot change this request's retries or handoff.
    assert!(runtime.set_key_upstream_protocol("key", UpstreamProtocolPolicy::Messages));
    assert_eq!(
        next.key_snapshot(&handshake).upstream_protocol,
        UpstreamProtocolPolicy::Responses
    );
    assert_eq!(
        next.compatible_routes(&runtime, &handshake)
            .unwrap_or_else(|error| panic!("{}", error.message))
            .1
            .len(),
        1
    );
    assert_eq!(
        active
            .compatible_routes(&runtime, &handshake)
            .unwrap_or_else(|error| panic!("{}", error.message))
            .1
            .len(),
        1
    );
    assert!(
        parse(fresh).is_err(),
        "new work must not use the handshake preference"
    );
    assert!(runtime.update_key_scope(
        "key",
        crate::CandidateScope {
            source_ids: Some(Default::default()),
            ..Default::default()
        },
    ));
    assert!(
        next.compatible_routes(&runtime, &handshake)
            .unwrap_or_else(|error| panic!("{}", error.message))
            .1
            .is_empty()
    );
}

#[test]
fn websocket_application_protocol_keeps_a_retained_model_outside_the_new_catalog() {
    let runtime = runtime();
    let handshake = runtime.authenticate_secret("local-secret").unwrap();
    let route = runtime
        .configured_executor_routes(&handshake, "upstream-model", WEBSOCKET_PROTOCOLS, false)
        .remove(0);
    let now = crate::unix_time_ms();
    runtime.bind_response_route_affinity(Some("resp_synthetic_owned"), &route, now);
    runtime.set_key_upstream_protocol("key", UpstreamProtocolPolicy::Messages);
    assert!(
        runtime
            .visible_models_for_secret("local-secret", WEBSOCKET_PROTOCOLS, now)
            .is_empty()
    );
    let request = ClientRequest::parse(
        &runtime,
        &handshake,
        &HeaderMap::new(),
        br#"{"model":"relay/upstream-model","previous_response_id":"resp_synthetic_owned","input":"Synthetic continuation"}"#,
    ).unwrap_or_else(|error| panic!("{}", error.message));
    assert_eq!(request.upstream_protocol, UpstreamProtocolPolicy::Messages);
    let routes = request
        .compatible_routes(&runtime, &handshake)
        .unwrap_or_else(|error| panic!("{}", error.message))
        .1;
    assert_eq!(routes.len(), 1);
    assert_eq!(routes["source"].adapter, route.adapter);
    assert!(
        ClientRequest::parse(
            &runtime,
            &handshake,
            &HeaderMap::new(),
            br#"{"model":"relay/upstream-model","input":"New independent conversation"}"#,
        )
        .is_err()
    );
}
