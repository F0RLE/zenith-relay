use super::{AdapterError, AdapterResult, Value};

/// Keeping all thinking matches the converter's full-history invariant.
/// Editing or clearing context still requires the original native protocol.
pub(super) fn validate(request: &Value) -> AdapterResult<()> {
    let Some(control) = request
        .get("context_management")
        .filter(|value| !value.is_null())
    else {
        return Ok(());
    };
    let preserves_all = control.as_object().is_some_and(|fields| {
        fields.len() == 1
            && fields
                .get("edits")
                .and_then(Value::as_array)
                .is_some_and(|edits| {
                    edits.len() == 1
                        && edits[0].as_object().is_some_and(|edit| {
                            edit.len() == 2
                                && edit.get("type").and_then(Value::as_str)
                                    == Some("clear_thinking_20251015")
                                && edit.get("keep").and_then(Value::as_str) == Some("all")
                        })
                })
    });
    if preserves_all {
        Ok(())
    } else {
        Err(AdapterError::parameter_unsupported_for(
            "context_management",
        ))
    }
}
