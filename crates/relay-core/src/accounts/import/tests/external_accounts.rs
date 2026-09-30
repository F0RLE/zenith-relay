use super::*;

#[test]
fn parses_sub2api_agent_identity_accounts() {
    let input = serde_json::json!({
        "type": "sub2api-data",
        "version": 1,
        "accounts": [{
            "name": "Agent account",
            "platform": "openai",
            "type": "oauth",
            "credentials": {
                "auth_mode": "agentIdentity",
                "agent_private_key": "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g",
                "agent_runtime_id": "runtime-test",
                "task_id": "task-test",
                "chatgpt_account_id": "account-test",
                "email": "agent@example.test"
            }
        }]
    })
    .to_string();
    let parsed = parse_import(&input, Some("sub2api.json"), &[]).unwrap();

    assert_eq!(parsed.items.len(), 1);
    assert_eq!(
        parsed.preview.rows[0].auth_mode,
        ImportAuthMode::AgentIdentity
    );
    assert_eq!(parsed.preview.rows[0].status, ImportPreviewStatus::Ready);
    assert_eq!(parsed.items[0].account_id.as_deref(), Some("account-test"));
    assert!(parsed.items[0].secrets().access_token().is_none());
    assert!(parsed.items[0].secrets().agent_private_key().is_some());
}

#[test]
fn parses_cockpit_and_sub2api_nested_credential_shapes() {
    let api_source = parse_import(
        &format!(
            r#"{{"type":"sub2api-data","version":1,"accounts":[{{"name":"Nested API source","type":"apikey","credentials":{{"api_key":"{API_KEY}","base_url":"https://api.example.test/v1?ignored=1","protocol":"responses"}}}}]}}"#
        ),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(api_source.items.len(), 1);
    assert_eq!(api_source.preview.rows[0].auth_mode, ImportAuthMode::ApiKey);
    assert_eq!(
        api_source.items[0].base_url.as_deref(),
        Some("https://api.example.test/v1")
    );
    assert_eq!(api_source.items[0].protocol.as_deref(), Some("responses"));

    let nested_identity = parse_import(
        r#"{
            "auth_mode":"agentIdentity",
            "credentials":{
                "agent_identity":{
                    "agent_private_key":"MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g",
                    "agent_runtime_id":"runtime-nested",
                    "task_id":"task-nested",
                    "chatgpt_account_id":"account-nested",
                    "chatgpt_user_id":"user-nested",
                    "email":"nested@example.test",
                    "chatgpt_account_is_fedramp":true
                }
            }
        }"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(nested_identity.items.len(), 1);
    assert_eq!(
        nested_identity.preview.rows[0].auth_mode,
        ImportAuthMode::AgentIdentity
    );
    assert_eq!(
        nested_identity.items[0].account_id.as_deref(),
        Some("account-nested")
    );
    assert_eq!(
        nested_identity.items[0].chatgpt_user_id.as_deref(),
        Some("user-nested")
    );
    assert!(nested_identity.items[0].account_is_fedramp);
    assert!(nested_identity.items[0]
        .secrets()
        .agent_private_key()
        .is_some());

    let cockpit_tokens = parse_import(
        &format!(
            r#"[{{"personal_access_token":"{ACCESS}"}},{{"credentials":{{"headers":{{"authorization":"Bearer {ACCESS}-header"}}}}}}]"#
        ),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(cockpit_tokens.items.len(), 2);
    assert_eq!(
        cockpit_tokens.items[0].secrets().access_token(),
        Some(ACCESS)
    );
    assert_eq!(
        cockpit_tokens.items[1].secrets().access_token(),
        Some("access-super-secret-header")
    );
    let preview = serde_json::to_string(&cockpit_tokens.preview).unwrap();
    assert!(!preview.contains(ACCESS));

    let cockpit_profile = parse_import(
        &format!(
            r#"{{"auth_mode":"personal_access_token","openai_auth_mode":"personal_access_token","personal_access_token":"{ACCESS}","api_wire_api":"responses","auth_file_plan_type":"plus","account_name":"Cockpit profile"}}"#
        ),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(
        cockpit_profile.preview.rows[0].auth_mode,
        ImportAuthMode::ImportedToken
    );
    assert_eq!(
        cockpit_profile.items[0].protocol.as_deref(),
        Some("responses")
    );
    assert_eq!(
        cockpit_profile.preview.rows[0].plan.as_deref(),
        Some("plus")
    );
    assert_eq!(cockpit_profile.preview.rows[0].label, "Cockpit profile");
    assert!(!cockpit_profile.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::UnknownAuthMode));
}

#[test]
fn imports_bounded_cockpit_tags_without_leaking_secrets() {
    let input = format!(
        r#"{{
            "type":"codex",
            "account_name":"Named account",
            "tags":[" work ","team","work","","{ACCESS}","bad\nvalue",7],
            "access_token":"{ACCESS}",
            "account_id":"acct-tag"
        }}"#
    );
    let parsed = parse_import(&input, None, &[]).unwrap();

    assert_eq!(
        parsed.items[0].tags,
        BTreeSet::from(["team".to_string(), "work".to_string()])
    );
    assert_eq!(parsed.preview.rows[0].label, "Named account");
    assert!(parsed.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::InvalidMetadataIgnored));

    let debug = format!("{:?}", parsed.items[0]);
    assert!(debug.contains("tag_count"));
    assert!(!debug.contains(ACCESS));
}

#[test]
fn portable_bundle_rejects_non_openai_rows_without_rejecting_openai_rows() {
    let parsed = parse_import(
        &format!(
            r#"{{"type":"sub2api-data","version":1,"accounts":[
                {{"name":"Other provider","platform":"anthropic","type":"oauth","credentials":{{"access_token":"{ACCESS}-other"}}}},
                {{"name":"OpenAI","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}-openai"}}}}
            ]}}"#
        ),
        None,
        &[],
    )
    .unwrap();

    assert_eq!(parsed.preview.rows.len(), 2);
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.preview.rows[0].status, ImportPreviewStatus::Invalid);
    assert_eq!(
        parsed.preview.rows[0]
            .error
            .as_ref()
            .map(|error| error.code),
        Some(ImportIssueCode::UnsupportedValue)
    );
    assert_eq!(parsed.preview.rows[1].auth_mode, ImportAuthMode::OAuth);
}
