use super::*;

fn response() -> Value {
    json!({"id":"synthetic-response","status":"completed","model":"synthetic-model",
        "output":[{"type":"message","role":"assistant","content":[{
            "type":"output_text","text":"Synthetic answer"}]}]})
}

#[test]
fn replay_rejection_is_sticky_after_error_or_foreign_model() {
    for event in [
        json!({"type":"error","error":{"code":"synthetic_provider_error","message":"Synthetic error"}}),
        json!({"type":"response.failed","response":{"status":"failed","error":{
            "code":"synthetic_provider_error","message":"Synthetic error"}}}),
        json!({"type":"response.created","response":{"model":"synthetic-other-model"}}),
        json!({"type":"response.completed","response":{"model":"synthetic-model-degrade2"}}),
        json!({"type":"response.completed","response":response(),"model":"synthetic-other-model"}),
    ] {
        let mut capture = NativeReplayCapture::default();
        capture.validate_for_replay(&event, Some("synthetic-model"));
        capture.validate_for_replay(
            &json!({"type":"response.completed","response":response()}),
            Some("synthetic-model"),
        );
        assert!(capture.finish(Some(response()), None).is_none());
    }
}

#[test]
fn replay_model_validation_accepts_missing_metadata_snapshots_and_generated_model_names() {
    for event in [
        json!({"type":"response.output_text.delta","delta":"synthetic-other-model degrade2"}),
        json!({"type":"response.created","response":{"model":"synthetic-model"}}),
        json!({"type":"response.created","response":{"model":"openai/synthetic-model-2026-11-12"}}),
        json!({"type":"response.completed","response":response()}),
    ] {
        let mut capture = NativeReplayCapture::default();
        capture.validate_for_replay(&event, Some("synthetic-model"));
        assert_eq!(capture.finish(Some(response()), None), Some(response()));
    }
}
