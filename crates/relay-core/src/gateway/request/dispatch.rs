use super::*;

pub(in crate::gateway) async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::Responses).await
}

pub(in crate::gateway) async fn chat_completions(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute_client_request(runtime, request, WireApi::ChatCompletions).await
}

pub(in crate::gateway) async fn messages(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    super::super::messages::native_messages_error_response(
        execute_client_request(runtime, request, WireApi::Messages).await,
    )
    .await
}

pub(in crate::gateway) async fn gemini(
    State(runtime): State<Arc<GatewayRuntime>>,
    axum::extract::Path(model_action): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let Some((model, stream)) = parse_gemini_model_action(&model_action) else {
        return super::super::errors::api_error(
            axum::http::StatusCode::NOT_FOUND,
            "Gemini endpoint must end with :generateContent or :streamGenerateContent",
            error_codes::INVALID_REQUEST,
        );
    };
    super::super::execution::execute_gemini_client_request(runtime, request, model, stream).await
}

fn parse_gemini_model_action(model_action_path: &str) -> Option<(String, bool)> {
    let (model_id, stream) =
        if let Some(model_id) = model_action_path.strip_suffix(":streamGenerateContent") {
            (model_id, true)
        } else {
            let model_id = model_action_path.strip_suffix(":generateContent")?;
            (model_id, false)
        };
    let model_id = model_id.strip_prefix("models/").unwrap_or(model_id).trim();
    crate::is_valid_model_id(model_id).then(|| (model_id.to_string(), stream))
}
