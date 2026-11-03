use super::*;

#[tokio::test]
async fn attachment_rotation_does_not_send_old_file_ids_with_new_credentials() {
    for stream in [false, true] {
        // A replacement login can restart the generation counter. Its exact
        // incarnation, not just that counter, owns the uploaded file reference.
        for generation in [1, 2] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            *crate::poison::mutex(&server.state.attachment_file_id) =
                Some("file-before-credential-rotation".to_string());
            *crate::poison::mutex(&server.state.generation_body) = final_response();
            let gate = Arc::new(Semaphore::new(0));
            crate::poison::mutex(&server.state.attachment_gates).push_back(gate.clone());
            let runtime = Arc::new(runtime(&server).await);
            let (base, task) = recovery::gateway(runtime.clone()).await;
            let mut request = catalog_request(stream);
            request["input"] = json!([{"role":"user","content":[{
                "type":"input_image",
                "image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
            }]}]);
            let client = retry_admission::bounded_client();
            let send = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send();
            let pending = tokio::spawn(send);
            tokio::time::timeout(
                Duration::from_secs(3),
                server.state.attachment_started.notified(),
            )
            .await
            .expect("the synthetic upload must reach its gate");
            runtime.chatgpt_accounts["account-1"]
                .token_authority
                .register(
                    "account-1",
                    TokenSet::access_only("synthetic-replacement-access", None, generation)
                        .unwrap(),
                    AccountAuthState::Active,
                )
                .await
                .unwrap();
            gate.add_permits(1);
            let response = pending.await.unwrap().unwrap();
            let status = response.status();
            let body = response.bytes().await.unwrap();
            task.abort();
            assert_eq!(
                server.state.generation_requests.load(Ordering::SeqCst),
                0,
                "old attachments must not cross credentials: stream={stream}, generation={generation}"
            );
            assert!(!status.is_success());
            assert_eq!(server.state.attachment_requests.load(Ordering::SeqCst), 1);
            assert!(!String::from_utf8_lossy(&body).contains("file-before-credential-rotation"));

            // A new explicit request uploads under the replacement credential;
            // it must not reuse the old response or poison the new cache scope.
            *crate::poison::mutex(&server.state.attachment_file_id) =
                Some("file-after-credential-rotation".to_string());
            let (base, task) = recovery::gateway(runtime).await;
            let response = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.bytes().await.unwrap();
            task.abort();
            assert_eq!(status, StatusCode::OK);
            assert!(body.windows(b"Ready".len()).any(|part| part == b"Ready"));
            assert_eq!(server.state.attachment_requests.load(Ordering::SeqCst), 2);
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            let bodies = crate::poison::mutex(&server.state.generation_bodies);
            assert!(
                bodies[0]
                    .to_string()
                    .contains("file-after-credential-rotation")
            );
            assert!(
                !bodies[0]
                    .to_string()
                    .contains("file-before-credential-rotation")
            );
        }
    }
}
