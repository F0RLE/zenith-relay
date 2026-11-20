use super::*;

#[path = "../../tests/fixture_files.rs"]
mod fixture_files;

const ORIGIN: &str = "https://provider.example/v1";
const SOURCE: Uuid = Uuid::from_u128(17);

fn entry(source: Uuid, endpoint: &str, ipv4: &str) -> String {
    format!(
        "\n[[network.outbound_policy.text_sources]]\nsource = '{source}'\nendpoint = '{endpoint}'\nipv4 = '{ipv4}'\n"
    )
}

fn config(approval: &str) -> Result<crate::config::Config, &'static str> {
    crate::config::Config::parse(&format!(
        "{}{approval}",
        include_str!("../../config.example.toml")
    ))
}

#[test]
fn text_approval_is_exact_and_never_bypasses_a_proxy() {
    let no_approval = OutboundPolicy::default();
    assert!(no_approval.text_route(SOURCE, ORIGIN, false).is_err());
    let c = config(&entry(SOURCE, ORIGIN, "1.1.1.1")).unwrap();
    let policy = c.network.outbound_policy;
    let approved = policy.text_route(SOURCE, ORIGIN, false).unwrap();
    assert_eq!(approved.direct_ipv4(), Some(Ipv4Addr::new(1, 1, 1, 1)));
    assert!(policy.text_route(Uuid::nil(), ORIGIN, false).is_err());
    assert!(policy.text_route(SOURCE, ORIGIN, true).is_err());
    assert!(approved.client(Some("http://127.0.0.1:90/")).is_err());
    for altered in [
        "https://another.example/v1",
        "https://provider.example/other",
        "http://127.0.0.1:8080/v1",
    ] {
        assert!(policy.text_route(SOURCE, altered, false).is_err());
    }
    let duplicate = format!(
        "{}{}",
        entry(SOURCE, ORIGIN, "1.1.1.1"),
        entry(SOURCE, ORIGIN, "1.0.0.1")
    );
    assert!(config(&duplicate).is_err());
}

#[test]
fn invalid_destinations_never_enter_the_policy() {
    for endpoint in [
        "http://provider.example/v1",
        "https://provider.example:443/v1",
        "https://provider.example:8443/v1",
        "https://provider.example./v1",
        "https://PROVIDER.example/v1",
        "https://127.0.0.1/v1",
        "https://[::1]/v1",
        "https://user:pass@provider.example/v1",
        "https://provider.example/v1?q=1",
        "https://provider.example/v1#part",
        "https://provider.example/v1/",
        "https://provider.example/a/../v1",
    ] {
        assert!(
            config(&entry(SOURCE, endpoint, "1.1.1.1")).is_err(),
            "{endpoint}"
        );
    }
    for ip in [
        "0.0.0.0",
        "10.2.3.4",
        "100.100.100.200",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.0.0.9",
        "192.0.2.1",
        "192.168.0.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "168.63.129.16",
        "::ffff:127.0.0.1",
    ] {
        assert!(config(&entry(SOURCE, ORIGIN, ip)).is_err(), "{ip}");
    }
}

#[test]
fn existing_loopback_fixture_still_has_a_strict_route() {
    let policy = OutboundPolicy::default();
    for endpoint in ["http://127.0.0.1:19999/v1", "http://[::1]:19999/v1"] {
        let local = policy.text_route(SOURCE, endpoint, true).unwrap();
        assert_eq!(local.direct_ipv4(), None);
        assert!(local.client(None).is_err());
        assert!(local.client(Some("http://proxy.example:19999/")).is_err());
        assert!(
            local
                .client(Some("http://test:fixture@127.0.0.1:1234/"))
                .is_ok()
        );
    }
    for invalid in [
        "http://example.invalid:19999/v1",
        "http://localhost:19999/v1",
        "http://user:pass@127.0.0.1:19999/v1",
        "http://127.0.0.1:19999/v1?route=other",
        "http://127.0.0.1:19999/v1#other",
        "http://127.0.0.1:19999/else",
        "http://127.0.0.1/v1",
    ] {
        assert!(
            policy.text_route(SOURCE, invalid, false).is_err(),
            "{invalid}"
        );
    }
}

#[tokio::test]
async fn resolver_has_one_address_and_no_fallback() {
    let address = "1.1.1.1:443".parse().unwrap();
    let resolver = PinnedDns(Some(("provider.example".into(), address)));
    let approved = resolver
        .resolve("provider.example".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(approved.collect::<Vec<_>>(), [address]);
    assert!(
        resolver
            .resolve("elsewhere.example".parse().unwrap())
            .await
            .is_err()
    );
    assert!(
        PinnedDns(None)
            .resolve("localhost".parse().unwrap())
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires Bun and OpenSSL CLI; uses an isolated loopback TLS fixture"]
async fn qualified_client_verifies_tls_and_keeps_its_pin_after_failure() {
    use reqwest::Certificate;
    use serde_json::{Value, json};
    use std::{
        fs,
        future::{Future, ready},
        pin::Pin,
        process::{Child, Command},
    };
    use zenith_relay_core::{
        ProviderSource, WireApi,
        gateway::hosted::{
            ClaimError, HostedAttempt, HostedBodyBinding, HostedDispatchAuthority,
            HostedExecutionError, HostedTextRuntime,
        },
        observed_responses_models,
    };

    struct Fixture(Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    struct Authority;
    impl HostedDispatchAuthority for Authority {
        fn claim<'a>(
            &'a self,
            attempt: &'a HostedAttempt,
        ) -> Pin<Box<dyn Future<Output = Result<(), ClaimError>> + Send + 'a>> {
            Box::pin(ready(if attempt.attempt_id == "denied" {
                Err(ClaimError::Denied)
            } else {
                Ok(())
            }))
        }
    }

    let dir = tempfile::Builder::new()
        .prefix("relay-hosted-")
        .tempdir()
        .unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let status = fixture_files::openssl()
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=fixture.test",
            "-addext",
            "subjectAltName=DNS:fixture.test",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-addext",
            "extendedKeyUsage=serverAuth",
            "-out",
        ])
        .arg(&cert)
        .arg("-keyout")
        .arg(&key)
        .output()
        .unwrap()
        .status;
    assert!(status.success(), "cannot generate fixture certificate");
    let leaf = Certificate::from_pem(&fs::read(&cert).unwrap()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let origin = format!("https://fixture.test:{}", address.port());
    // Only this test supplies a private CA and a loopback address to HTTPS.
    let client = https_client("fixture.test", address)
        .tls_certs_only([leaf.clone()])
        .build()
        .unwrap();
    let source = ProviderSource {
        id: "test-source".into(),
        name: "Fixture".into(),
        base_url: format!("{origin}/v1"),
        wire_api: WireApi::Responses,
        models: vec!["fixture-text".into()],
        api_key: "fixture-provider".into(),
        proxy_url: None,
    };
    let engine = HostedTextRuntime::new(source.clone(), client.clone()).unwrap();
    let execute = |id: &'static str| {
        let engine = &engine;
        let source = &source;
        async move {
            let body = serde_json::to_vec(&json!({
                "model":"fixture-text", "input":id, "max_output_tokens":8
            }))
            .unwrap();
            let attempt = HostedAttempt {
                workspace_id: "workspace".into(),
                key_id: "key".into(),
                request_id: id.into(),
                attempt_id: id.into(),
                source_id: source.id.clone(),
                model_id: "fixture-text".into(),
                authority_revision: 1,
                tariff_revision: 1,
                bound_micro_usd: 100,
                body_binding: HostedBodyBinding::from_bytes(&body),
                max_input_bytes: 64,
                max_output_tokens: 8,
            };
            engine
                .execute(
                    &attempt,
                    axum::http::Request::post("/v1/responses")
                        .body(body.as_slice())
                        .unwrap(),
                    &Authority,
                )
                .await
        }
    };
    assert!(execute("offline").await.is_err());

    let mut fixture = Fixture(
        Command::new("bun")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .arg("tests/fixtures/pinned-text.mjs")
            .arg(address.port().to_string())
            .arg(&cert)
            .arg(&key)
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while tokio::net::TcpStream::connect(address).await.is_err() {
            assert!(fixture.0.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("HTTPS fixture ready");

    assert_eq!(
        observed_responses_models(&client, &source).await.unwrap(),
        ["fixture-text"]
    );
    assert_eq!(
        execute("denied").await,
        Err(HostedExecutionError::Claim(ClaimError::Denied))
    );
    assert_eq!(execute("ready").await.unwrap()["status"], "completed");
    assert_eq!(
        execute("redirect").await,
        Err(HostedExecutionError::UpstreamStatus(307))
    );
    let wrong_name = https_client("wrong.test", address)
        .tls_certs_only([leaf])
        .build()
        .unwrap();
    assert!(
        wrong_name
            .get(format!("https://wrong.test:{}/v1/models", address.port()))
            .send()
            .await
            .unwrap_err()
            .is_connect()
    );
    let stats = client.get(format!("{origin}/counts")).send().await.unwrap();
    assert_eq!(stats.remote_addr(), Some(address));
    assert_eq!(
        stats.json::<Value>().await.unwrap(),
        json!({"catalog":1, "responses":2, "unexpected":0})
    );
}
