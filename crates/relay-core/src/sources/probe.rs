use super::{
    CapabilityOrigin, CapabilityStatus, ModelEndpointCapability, ProtocolFeature, ProviderSource,
    SourceConnector, SourceProtocolBinding, WireApi,
};
use crate::{error_codes, transport::collect_limited, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Duration};

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PROBE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceProbeInput {
    pub model_id: String,
    pub wire_api: WireApi,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProbeResult {
    pub capability: ModelEndpointCapability,
    pub revision: u64,
    pub http_status: Option<u16>,
    pub error_code: Option<String>,
}

/// This operation is called only by an explicit management action. It never
/// accepts a user prompt, follows redirects, or returns upstream response data.
pub async fn probe_source_generation(
    source: &ProviderSource,
    input: &SourceProbeInput,
) -> Result<SourceProbeResult> {
    source.validate()?;
    let model = source
        .models
        .iter()
        .find(|model| model.eq_ignore_ascii_case(input.model_id.trim()))
        .ok_or_else(|| Error::Validation("probe model must belong to the source catalog".into()))?;
    let binding = SourceProtocolBinding::legacy(input.wire_api, std::slice::from_ref(model));
    let connector = SourceConnector::new(source, std::slice::from_ref(&binding))?;
    let endpoint = connector
        .endpoint(binding.key(), model, false)
        .ok_or_else(|| Error::Validation("probe endpoint is invalid".into()))?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(PROBE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let (header, authorization) = connector.authorization_for_binding(&binding);
    let request = client
        .post(endpoint)
        .header(header, authorization)
        .headers(connector.protocol_headers_for_binding(&binding))
        .json(&probe_body(input.wire_api, model));
    let mut result = SourceProbeResult {
        capability: ModelEndpointCapability {
            model_id: model.clone(),
            upstream_wire_api: input.wire_api,
            status: CapabilityStatus::Unknown,
            origin: CapabilityOrigin::GenerationProbe,
            checked_at_ms: crate::unix_time_ms(),
            features: BTreeMap::new(),
            reasoning_efforts: vec![],
        },
        revision: input.expected_revision,
        http_status: None,
        error_code: Some(error_codes::SOURCE_PROBE_UNAVAILABLE.into()),
    };
    let exchange = async {
        let response = request.send().await?;
        let status = response.status();
        let body = collect_limited(response, MAX_PROBE_BYTES).await?;
        Ok::<_, Error>((status, body))
    };
    if let Ok(Ok((status, body))) = tokio::time::timeout(PROBE_TIMEOUT, exchange).await {
        result.http_status = Some(status.as_u16());
        if status.is_success() {
            let valid = serde_json::from_slice::<Value>(&body)
                .ok()
                .is_some_and(|body| valid_text_response(input.wire_api, &body));
            if valid {
                result.capability.status = CapabilityStatus::Confirmed;
                result
                    .capability
                    .features
                    .insert(ProtocolFeature::Text, CapabilityStatus::Confirmed);
                result.error_code = None;
            } else {
                result.error_code = Some(error_codes::SOURCE_PROBE_INVALID_RESPONSE.into());
            }
        } else if matches!(status.as_u16(), 404 | 405) {
            result.capability.status = CapabilityStatus::Unsupported;
            result.error_code = Some(error_codes::SOURCE_PROBE_UNSUPPORTED.into());
        }
    }
    result.capability.checked_at_ms = crate::unix_time_ms();
    Ok(result)
}

fn probe_body(protocol: WireApi, model: &str) -> Value {
    const TEXT: &str = "Reply with OK.";
    match protocol {
        WireApi::Responses => {
            json!({"model":model,"input":TEXT,"max_output_tokens":64,"store":false,"stream":false})
        }
        WireApi::ChatCompletions => {
            json!({"model":model,"messages":[{"role":"user","content":TEXT}],"max_tokens":64,"stream":false})
        }
        WireApi::Messages => {
            json!({"model":model,"messages":[{"role":"user","content":TEXT}],"max_tokens":64,"stream":false})
        }
        WireApi::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":TEXT}]}],"generationConfig":{"maxOutputTokens":64}})
        }
    }
}

fn valid_text_response(protocol: WireApi, body: &Value) -> bool {
    let nonempty = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    };
    if body.get("error").is_some_and(|e| !e.is_null()) {
        return false;
    }
    match protocol {
        WireApi::Responses => {
            body.get("status").and_then(Value::as_str) == Some("completed")
                && body
                    .get("output")
                    .and_then(Value::as_array)
                    .is_some_and(|output| {
                        output.iter().any(|item| {
                            item.get("type").and_then(Value::as_str) == Some("message")
                                && item.get("content").and_then(Value::as_array).is_some_and(
                                    |content| {
                                        content.iter().any(|part| {
                                            part.get("type").and_then(Value::as_str)
                                                == Some("output_text")
                                                && nonempty(part.get("text"))
                                        })
                                    },
                                )
                        })
                    })
        }
        WireApi::ChatCompletions => {
            body.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                == Some("stop")
                && nonempty(body.pointer("/choices/0/message/content"))
        }
        WireApi::Messages => {
            body.get("type").and_then(Value::as_str) == Some("message")
                && body.get("stop_reason").and_then(Value::as_str) == Some("end_turn")
                && body
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|content| {
                        content.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) == Some("text")
                                && nonempty(part.get("text"))
                        })
                    })
        }
        WireApi::Gemini => {
            body.pointer("/candidates/0/finishReason")
                .and_then(Value::as_str)
                == Some("STOP")
                && body
                    .pointer("/candidates/0/content/parts")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("thought").and_then(Value::as_bool) != Some(true)
                                && nonempty(part.get("text"))
                        })
                    })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};

    #[tokio::test]
    async fn confirms_only_text_and_keeps_auth_and_transient_failures_unknown() {
        for status in [200, 401, 403, 429, 500, 503, 404, 405] {
            let app = Router::new().route("/v1/chat/completions", post(move |Json(body): Json<Value>| async move {
                assert_eq!(body["max_tokens"], 64);
                assert_eq!(body["messages"][0]["content"], "Reply with OK.");
                (axum::http::StatusCode::from_u16(status).unwrap(), Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"OK"}}]})))
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let source = ProviderSource {
                id: "probe-test".into(),
                name: "Probe test".into(),
                base_url: format!("http://{address}/v1"),
                api_key: "synthetic-test-key".into(),
                wire_api: WireApi::ChatCompletions,
                models: vec!["test-model".into()],
            };
            let input = SourceProbeInput {
                model_id: "test-model".into(),
                wire_api: WireApi::ChatCompletions,
                expected_revision: 3,
            };
            let result = probe_source_generation(&source, &input).await.unwrap();
            assert_eq!(
                result.capability.status,
                match status {
                    200 => CapabilityStatus::Confirmed,
                    404 | 405 => CapabilityStatus::Unsupported,
                    _ => CapabilityStatus::Unknown,
                }
            );
            assert!(!result
                .capability
                .features
                .contains_key(&ProtocolFeature::Streaming));
            assert_eq!(result.revision, 3);
            server.abort();
        }
    }

    #[test]
    fn arbitrary_json_and_incomplete_responses_never_confirm_a_route() {
        for protocol in WireApi::ALL {
            assert!(!valid_text_response(
                protocol,
                &json!({"data":[{"id":"model"}]})
            ));
            assert!(!valid_text_response(protocol, &json!({})));
        }
        assert!(!valid_text_response(
            WireApi::Responses,
            &json!({"status":"incomplete","output":[]})
        ));
    }
}
