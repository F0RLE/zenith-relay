use super::*;
use crate::runtime::siwc_socket::PreparedSocket;
use futures_util::FutureExt;

#[tokio::test]
async fn queued_preparation_rechecks_credentials_and_budget_before_another_handshake() {
    for replace_credentials in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        let runtime = runtime(&siwc, &native).await;
        let key = runtime.authenticate_secret("local-secret").unwrap();
        let budget = SharedRequestBudget::for_incoming_request(3);
        let (route, lease) = runtime
            .select_and_reserve_with_budget(
                &key,
                "gpt-test",
                &[WireApi::Responses],
                &Default::default(),
                (None, None),
                crate::unix_time_ms(),
                &budget,
            )
            .await
            .unwrap();
        let candidate = "synthetic-account-0";
        assert_eq!(route.candidate_id, candidate);
        let prepared = runtime
            .prepare_authorization(candidate, crate::unix_time_ms())
            .await
            .unwrap();
        let request = reqwest::Client::new()
            .post(siwc.responses_url.clone())
            .json(&json!({"model":"gpt-test","store":false,"stream":true,
                "input":[{"role":"user","content":"synthetic"}]}))
            .build()
            .unwrap();
        let mut event = crate::gateway::test_support::test_usage_event();
        event.client_context_id = Some("synthetic-thread".into());
        let dispatch = AuthorizationDispatch {
            client_version: None,
            identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
            turn_scope: None,
            budget: Some(&budget),
            lease: Some(&lease),
            usage_event: Some((&event, std::time::Instant::now())),
        };
        let PreparedSocket::Ready(owner) = runtime
            .prepare_siwc_socket(candidate, &request, &prepared, dispatch, None)
            .await
            .unwrap()
        else {
            panic!("a synthetic SIWC session must be prepared");
        };
        let mut waiting =
            Box::pin(runtime.prepare_siwc_socket(candidate, &request, &prepared, dispatch, None));
        assert!(waiting.as_mut().now_or_never().is_none());
        if replace_credentials {
            let authority = &runtime.account_state.executors[candidate].token_authority;
            let tokens = authority.tokens(candidate).await.unwrap();
            authority
                .register(candidate, tokens, AccountAuthState::Active)
                .await
                .unwrap();
        } else {
            budget.with_budget(|budget| {
                budget
                    .observe_execution(crate::scheduler::rotation::ExecutionObservation::unknown())
            });
        }
        drop(owner);
        let result = waiting.await;
        if replace_credentials {
            assert!(matches!(result, Err(AuthorizedRequestError::NotReplayable)));
        } else {
            assert!(matches!(
                result,
                Err(AuthorizedRequestError::DispatchBudgetExhausted)
            ));
        }
        assert_eq!(budget.dispatches(), 0);
        assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 1);
        assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    }
}
