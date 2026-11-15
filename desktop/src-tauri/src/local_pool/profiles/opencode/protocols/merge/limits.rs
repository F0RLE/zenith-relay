use serde_json::Value;
use zenith_relay_core::{ModelTokenLimits, model_metadata::ModelCapabilities};

/// User windows remain preferences within the current generated hard ceilings.
/// Without a known ceiling, retain the user's existing configuration.
pub(super) fn merge(generated: Option<&Value>, saved: &Value) -> Value {
    let Some(generated) = generated.and_then(Value::as_object) else {
        return saved.clone();
    };
    let positive = |value: Option<&Value>| value.and_then(Value::as_u64).filter(|value| *value > 0);
    let ceilings = ModelTokenLimits {
        context: positive(generated.get("context")),
        input: positive(generated.get("input")),
        output: positive(generated.get("output")),
    };
    if ceilings == ModelTokenLimits::default() {
        return saved.clone();
    }
    let Some(saved) = saved.as_object() else {
        return Value::Object(generated.clone());
    };
    let mut capabilities = ModelCapabilities {
        context_limit: positive(saved.get("context")),
        input_limit: positive(saved.get("input")),
        output_limit: positive(saved.get("output")),
        ..Default::default()
    };
    ceilings.constrain(&mut capabilities);
    let mut merged = saved.clone();
    for (name, value) in [
        ("context", capabilities.context_limit),
        ("input", capabilities.input_limit),
        ("output", capabilities.output_limit),
    ] {
        if let Some(value) = value {
            merged.insert(name.into(), value.into());
        }
    }
    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn user_windows_survive_only_within_current_generated_ceilings() {
        let generated = json!({"context":32000,"input":24000,"output":8000});
        let small = json!({"context":16000,"input":12000,"output":4000,"custom":"retained"});
        assert_eq!(merge(Some(&generated), &small), small);
        assert_eq!(merge(None, &small), small);
        assert_eq!(merge(Some(&json!({})), &small), small);
        let large = json!({"context":128000,"input":96000,"output":32000});
        assert_eq!(merge(Some(&generated), &large), generated);
        assert_eq!(
            merge(Some(&generated), &json!({"context":4000})),
            json!({"context":4000,"input":4000,"output":4000})
        );
    }

    #[test]
    fn invalid_saved_bounds_cannot_replace_known_positive_limits() {
        let generated = json!({"context":32000,"input":24000,"output":8000});
        for invalid in [json!(0), json!(-1), json!(1.5), json!("32000"), Value::Null] {
            let saved = json!({"context":invalid,"input":invalid,"output":invalid});
            assert_eq!(merge(Some(&generated), &saved), generated);
            assert_eq!(merge(None, &saved), saved);
        }
        assert_eq!(merge(Some(&generated), &json!("invalid")), generated);
    }
}
