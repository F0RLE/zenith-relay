use super::*;
use crate::local_pool::{commands::pool, store::secret_store};

#[test]
fn network_addresses_preserve_lan_alongside_vpn_and_ignore_unusable_interfaces() {
    use InterfaceOperationalState::{Down, Unknown, Up};
    let lan = [
        "192.168.1.20/24".parse().unwrap(),
        "192.168.1.20/24".parse().unwrap(),
    ];
    let vpn = ["10.8.0.2/32".parse().unwrap()];
    let other = [
        "127.0.0.1/8",
        "169.254.1.2/16",
        "203.0.113.1/24",
        "::1/128",
        "fd00::1/64",
    ]
    .map(|ip| ip.parse().unwrap());
    let interfaces = [
        ("VPN", Up, vpn.as_slice()),
        ("Wi-Fi", Down, lan.as_slice()),
        ("Ethernet", Up, lan.as_slice()),
        ("Other", Unknown, other.as_slice()),
    ];
    let addresses = network_addresses(interfaces, 15432);
    assert_eq!(
        addresses,
        vec![
            NetworkAddress {
                interface: "Ethernet".into(),
                base_url: "http://192.168.1.20:15432/v1".into()
            },
            NetworkAddress {
                interface: "VPN".into(),
                base_url: "http://10.8.0.2:15432/v1".into()
            },
        ]
    );
    assert_eq!(
        network_addresses(interfaces.into_iter().rev(), 15432),
        addresses
    );
    assert_eq!(
        network_addresses([("en0", Unknown, lan.as_slice())], 15432).len(),
        1
    );
    assert!(network_addresses([("Ethernet", Down, lan.as_slice())], 15432).is_empty());
}

#[tokio::test]
async fn network_access_is_opt_in_revision_checked_and_keeps_independent_keys() {
    let root = std::env::temp_dir().join(format!("relay-network-{}", uuid::Uuid::new_v4()));
    let state = DesktopState::open(root.clone()).unwrap();
    assert!(!snapshot(&state).unwrap().enabled);
    let local = pool::ensure_system_gateway_key(&state).unwrap();
    let local_secret = pool::ensure_local_gateway_key_secret(&local).unwrap();
    assert!(update(&state, true, false).await.unwrap().enabled);
    assert!(state.gateway.address().await.is_none());
    assert!(update(&state, false, false).await.is_err());
    let network = pool::ensure_lan_gateway_key(&state).unwrap();
    let network_secret = pool::ensure_local_gateway_key_secret(&network).unwrap();
    assert_ne!(local_secret, network_secret);
    assert_ne!(local.id, network.id);
    pool::retire_user_gateway_keys(&state).unwrap();
    assert_eq!(state.store().unwrap().keys().len(), 2);
    assert_eq!(
        secret_store::load(&local.secret_ref).unwrap(),
        Some(local_secret)
    );
    assert_eq!(
        secret_store::load(&network.secret_ref).unwrap(),
        Some(network_secret)
    );
    let persisted = state.store().unwrap().gateway().clone();
    assert_eq!(persisted.bind_scope, BindScope::Lan);
    assert_eq!(persisted.client_host, "127.0.0.1");
    assert!(!update(&state, false, true).await.unwrap().enabled);
    secret_store::delete(&local.secret_ref).unwrap();
    secret_store::delete(&network.secret_ref).unwrap();
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn network_scope_restarts_same_pool_without_changing_local_address_or_key() {
    let root = std::env::temp_dir().join(format!("relay-network-runtime-{}", uuid::Uuid::new_v4()));
    let state = DesktopState::open(root.clone()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut settings = state.store().unwrap().gateway().clone();
    settings.port = port;
    state.store().unwrap().replace_gateway(settings).unwrap();
    let local = pool::ensure_system_gateway_key(&state).unwrap();
    let local_secret = pool::ensure_local_gateway_key_secret(&local).unwrap();
    let runtime = super::super::super::runtime_from_store(&state)
        .await
        .unwrap();
    let original = state.gateway.start(runtime, port).await.unwrap();

    update(&state, true, false).await.unwrap();
    assert_eq!(state.gateway.address().await, Some(original));
    let network = pool::ensure_lan_gateway_key(&state).unwrap();
    let network_secret = pool::ensure_local_gateway_key_secret(&network).unwrap();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();
    for secret in [&local_secret, &network_secret] {
        let response = client
            .get(format!("http://{original}/v1/models"))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.bytes().await.unwrap();
    }

    update(&state, false, true).await.unwrap();
    assert_eq!(state.gateway.address().await, Some(original));
    for (secret, status) in [
        (&local_secret, reqwest::StatusCode::OK),
        (&network_secret, reqwest::StatusCode::UNAUTHORIZED),
    ] {
        let response = client
            .get(format!("http://{original}/v1/models"))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        response.bytes().await.unwrap();
    }
    drop(client);
    state.gateway.stop().await;
    secret_store::delete(&local.secret_ref).unwrap();
    secret_store::delete(&network.secret_ref).unwrap();
    let telemetry = std::sync::Arc::downgrade(&state.telemetry);
    drop(state);
    assert_eq!(
        telemetry.strong_count(),
        0,
        "gateway shutdown retained telemetry"
    );
    std::fs::remove_dir_all(&root)
        .unwrap_or_else(|error| panic!("failed to remove test storage {root:?}: {error}"));
}
