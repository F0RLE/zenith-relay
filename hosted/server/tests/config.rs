use zenith_relay_hosted::config::{Config, validate_database_url};

const EXAMPLE: &str = include_str!("../config.example.toml");

#[test]
fn customer_authority_and_endpoints_are_operator_pinned() {
    let section = "[customer_oidc]\nid='af47d252-403b-4ed8-9845-ac609c278730'\nissuer='https://id.example'\nclient_id='fixture'\n\
        authorization_endpoint='https://id.example/auth'\ntoken_endpoint='https://id.example/token'\njwks_uri='https://id.example/keys'";
    let valid = format!("{EXAMPLE}\n{section}");
    assert!(Config::parse(&valid).is_ok());
    for (original, replacement) in [
        ("https://id.example/token", "https://other.example/token"),
        ("https://id.example/token", "https://id.example/token?x=1"),
        ("https://id.example/keys", "https://name@id.example/keys"),
        (
            "https://id.example/keys",
            "https://id.example/keys#fragment",
        ),
        ("https://id.example/", "http://id.example/"),
        ("https://id.example/token", "https://id.example/auth"),
        ("client_id='fixture'", "client_id=''"),
    ] {
        assert!(Config::parse(&valid.replace(original, replacement)).is_err());
    }
    let loopback = valid.replace("https://id.example", "http://127.0.0.1:9901");
    assert!(Config::parse(&loopback).is_ok());
    assert!(
        Config::parse(&loopback.replace("http://localhost:1443", "https://localhost")).is_err()
    );
}

#[test]
fn example_is_closed_loopback_and_config_cannot_open_service() {
    let c = Config::parse(EXAMPLE).unwrap();
    assert!(c.server.bind.ip().is_loopback());
    assert_eq!(c.branding.name, "Relay");
    for (before, after) in [
        ("bind = \"127.0.0.1", "bind = \"0.0.0.0"),
        ("http://localhost:1443", "http://example.org"),
        ("http://localhost:1443", "https://user@example.org"),
        ("http://localhost:1443", "https://example.org/path"),
        (
            "admin_rp_id = \"localhost\"",
            "admin_rp_id = \"another.example\"",
        ),
        ("max_connections = 4", "max_connections = 0"),
        ("lock_timeout_ms = 1000", "lock_timeout_ms = 6000"),
        ("[database]", "[database]\nurl = \"not-a-secret\""),
        ("name = \"Relay\"", "name = \"   \""),
        ("[server]", "[server]\nmode = \"live\""),
    ] {
        assert!(Config::parse(&EXAMPLE.replace(before, after)).is_err());
    }
}

#[test]
fn database_urls_cannot_override_identity_or_downgrade_remote_tls() {
    assert!(validate_database_url("postgres://reader@localhost/relay?sslmode=disable").is_ok());
    assert!(
        validate_database_url("postgres://reader@db.example/relay?sslmode=verify-full").is_ok()
    );
    for dsn in [
        "postgres://db.example/relay?sslmode=verify-full",
        "postgres://reader@localhost/relay?host=db.example",
        "postgres://reader@db.example/relay",
        "postgres://reader@db.example/relay?sslmode=verify-full&sslmode=disable",
        "http://reader@localhost/relay",
    ] {
        assert!(validate_database_url(dsn).is_err());
    }
}

#[test]
fn policy_links_are_optional_public_destinations_only() {
    assert!(Config::parse(EXAMPLE).unwrap().policies.is_empty());
    let with = |value: &str| Config::parse(&format!("{EXAMPLE}\nprivacy = {value:?}"));
    let c = with("https://policy.example").unwrap();
    assert_eq!(
        c.policies.privacy.as_deref(),
        Some("https://policy.example/")
    );
    for value in [
        "http://policy.example/",
        "https://fixture@policy.example/privacy",
        "https://policy.example/policy?private=value",
        "https://policy.example/#section",
        "data:text/html,example",
        "//policy.example/privacy",
    ] {
        assert!(with(value).is_err());
    }
    assert!(Config::parse(&format!("{EXAMPLE}\napi_key = 'invalid-field'")).is_err());
}

#[test]
fn capture_requires_all_caps_and_rejects_invalid_configured_values() {
    let parse =
        |settings: &str| Config::parse(&format!("{EXAMPLE}\n[diagnostics.capture]\n{settings}"));
    assert!(
        Config::parse(EXAMPLE)
            .unwrap()
            .diagnostics
            .limits()
            .is_none()
    );
    for incomplete in [
        "max_bytes=4096\nmax_events_per_second=10",
        "max_events=20\nmax_events_per_second=10",
        "max_events=20\nmax_bytes=4096",
    ] {
        assert!(parse(incomplete).unwrap().diagnostics.limits().is_none());
    }
    let caps = parse("max_events=20\nmax_bytes=4096\nmax_events_per_second=10")
        .unwrap()
        .diagnostics
        .limits()
        .unwrap();
    assert_eq!((caps.default_seconds, caps.max_seconds), (600, 1800));
    for invalid in [
        "max_events=0",
        "max_events=129",
        "max_bytes=127",
        "max_bytes=32769",
        "max_events_per_second=17",
        "max_seconds=1801",
        "max_seconds=300",
        "default_seconds=0",
        "max_events=1.5",
        "include_body=true",
    ] {
        assert!(parse(invalid).is_err(), "{invalid}");
    }
}
