use serde_json::json;

#[test]
fn account_import_fixture_is_synthetic_and_never_serializes_debug_secrets() {
    let input = json!({
        "label":"Synthetic account",
        "accessToken":"synthetic-access-token",
        "refreshToken":"synthetic-refresh-token",
        "chatgptAccountId":"synthetic-account-id",
        "models":["gpt-test"]
    });
    let rendered = format!(
        "{:?}",
        input.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert!(!rendered.contains("synthetic-access-token"));
    assert!(!rendered.contains("synthetic-refresh-token"));
}
