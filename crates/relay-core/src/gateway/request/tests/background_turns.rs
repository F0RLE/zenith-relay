use super::*;
use serde_json::json;

#[test]
fn background_classifier_requires_explicit_codex_marker() {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"model":"gpt-5.6-luna","reasoning":{"effort":"low"}})
        ),
        None
    );
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"request_type":"task_title"}"#),
    );
    assert_eq!(
        codex_background_request_kind(&headers, &json!({"model":"gpt-5.6-luna"})),
        Some(CODEX_TASK_TITLE)
    );
}
#[test]
fn background_classifier_accepts_exact_internal_prompt_prefixes() {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert("x-openai-subagent", HeaderValue::from_static("true"));
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"input":[{"content":[{"text":"Summarize the activity for this task"}]}]})
        ),
        Some(CODEX_ACTIVITY_SUMMARY)
    );
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"instructions":"Generate a concise title for this task"})
        ),
        Some(CODEX_TASK_TITLE)
    );
}
#[test]
fn background_classifier_does_not_use_prompt_text_without_marker() {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"instructions":"Summarize the activity for this task, then answer me"})
        ),
        None
    );
}
#[test]
fn background_classifier_recognizes_current_chatgpt_background_operations() {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("chatgpt desktop"));
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"request_kind":"turn","thread_source":"thread_title"}"#),
    );
    assert_eq!(
        codex_background_request_kind(&headers, &json!({"model":"gpt-6-luna"})),
        Some(CODEX_TASK_TITLE)
    );

    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"turn_trigger":"thread_summary"}"#),
    );
    assert_eq!(
        codex_background_request_kind(&headers, &json!({"model":"gpt-6-luna"})),
        Some(CODEX_ACTIVITY_SUMMARY)
    );

    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"client_metadata":{"thread_source":"thread_description"}})
        ),
        Some(CODEX_ACTIVITY_SUMMARY)
    );

    let mut websocket_headers = HeaderMap::new();
    websocket_headers.insert("originator", HeaderValue::from_static("chatgptdesktop"));
    assert_eq!(
        codex_background_request_kind(
            &websocket_headers,
            &json!({
                "client_metadata": {
                    "x-codex-turn-metadata": "{\"request_kind\":\"turn\",\"thread_source\":\"thread_title_reconsideration\"}"
                }
            })
        ),
        Some(CODEX_TASK_TITLE)
    );
}
#[test]
fn background_classifier_leaves_ordinary_chatgpt_turns_alone() {
    let mut plain = HeaderMap::new();
    plain.insert("originator", HeaderValue::from_static("chatgpt desktop"));
    assert_eq!(
        codex_background_request_kind(
            &plain,
            &json!({"instructions":"You write the one-line activity update displayed beneath an existing task"})
        ),
        None
    );

    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("chatgpt desktop"));
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(
            r#"{"request_kind":"turn","thread_source":"user","turn_trigger":"user"}"#,
        ),
    );
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({
                "model":"gpt-6-luna",
                "instructions":"Answer the question, then summarize the activity for this task",
                "input":[{"content":[{"text":"Generate a title for the report"}]}]
            })
        ),
        None
    );
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"request_kind":"compaction","thread_source":"user"}"#),
    );
    assert_eq!(
        codex_background_request_kind(&headers, &json!({"model":"gpt-6-luna"})),
        None
    );
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"request_kind":"prewarm"}"#),
    );
    assert_eq!(
        codex_background_request_kind(&headers, &json!({"model":"gpt-6-luna"})),
        None
    );
}
#[test]
fn background_classifier_accepts_current_background_prompt_prefixes() {
    let mut headers = HeaderMap::new();
    headers.insert("originator", HeaderValue::from_static("chatgpt desktop"));
    headers.insert(
        "x-codex-turn-metadata",
        HeaderValue::from_static(r#"{"request_kind":"turn"}"#),
    );
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"input":[{"content":[{"text":"You write the one-line activity update displayed beneath an existing task"}]}]})
        ),
        Some(CODEX_ACTIVITY_SUMMARY)
    );
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"instructions":"Generate a concise, single-line task title of at most 36 characters"})
        ),
        Some(CODEX_TASK_TITLE)
    );
    assert_eq!(
        codex_background_request_kind(
            &headers,
            &json!({"input":"You are a helpful assistant. You will be presented with a user prompt, and your job is to provide a short title for a task"})
        ),
        Some(CODEX_TASK_TITLE)
    );
}
