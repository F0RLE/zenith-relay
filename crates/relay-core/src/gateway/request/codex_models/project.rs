use super::*;

#[cfg(test)]
pub(in crate::gateway) fn build_codex_models_response(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    upstream: Option<&Value>,
) -> Option<Value> {
    build_codex_models_response_from_manifests(
        runtime,
        key,
        visible_models,
        upstream
            .cloned()
            .into_iter()
            .map(|manifest| (String::new(), manifest)),
    )
}

pub(super) fn build_codex_models_response_from_manifests(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    visible_models: &[String],
    upstreams: impl IntoIterator<Item = (String, Value)>,
) -> Option<Value> {
    let upstream_manifests = upstreams.into_iter().collect::<Vec<_>>();
    let visible = visible_models
        .iter()
        .filter_map(|display_id| {
            runtime.resolve_model(key, display_id).map(|upstream_id| {
                (
                    crate::model_id_key(&upstream_id),
                    (upstream_id, display_id.clone()),
                )
            })
        })
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return None;
    }
    // Source models remain provider-agnostic in the runtime. The picker is the
    // presentation boundary: it groups familiar model IDs while the upstream
    // account catalog supplies transport templates for those same IDs.
    let mut upstream_by_model = HashMap::<String, Vec<(String, Value)>>::new();
    for (candidate_id, manifest) in &upstream_manifests {
        let Some(models) = upstream_codex_models(manifest) else {
            continue;
        };
        for model in models {
            let Some(object) = model.as_object() else {
                continue;
            };
            let Some(slug) = object.get("slug").and_then(Value::as_str).map(str::trim) else {
                continue;
            };
            if !is_valid_model_id(slug)
                || !codex_model_is_picker_eligible_for(
                    slug,
                    runtime.block_degraded_routes_enabled(),
                )
                || object
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("hide"))
            {
                continue;
            }
            let normalized = crate::model_id_key(slug);
            if visible
                .iter()
                .any(|(upstream_id, _)| upstream_id == &normalized)
            {
                upstream_by_model
                    .entry(normalized)
                    .or_default()
                    .push((candidate_id.clone(), Value::Object(object.clone())));
            }
        }
    }

    let mut models = Vec::with_capacity(visible.len());
    for (index, (normalized, (upstream_id, display_id))) in visible.into_iter().enumerate() {
        if !codex_model_is_picker_eligible_for(
            &upstream_id,
            runtime.block_degraded_routes_enabled(),
        ) {
            continue;
        }
        let priority = crate::CODEX_CATALOG_PRIORITY_BASE.saturating_add(index as u64);
        // These helpers accept the client-facing model spelling because they
        // resolve the key prefix internally. Passing the already-resolved
        // upstream id would make prefixed keys look like API-only routes.
        let native_account_ids = runtime.codex_model_native_responses_account_ids(key, &display_id);
        let has_native_account_route = !native_account_ids.is_empty();
        let capabilities = runtime.model_capabilities(&upstream_id);
        let native_entries = upstream_by_model
            .get(&normalized)
            .into_iter()
            .flatten()
            .filter(|(candidate_id, _)| {
                candidate_id.is_empty()
                    || native_account_ids
                        .iter()
                        .any(|account_id| account_id == candidate_id)
            });
        // Only the exact owning account's card can supply native transport.
        // Model identity is independent: a missing card must not rename a GPT
        // model or copy another account/model's transport controls.
        let native_catalog_model = has_native_account_route
            .then(|| {
                native_entries.clone().find_map(|(_, entry)| {
                    // Ignore participant semantic fields before validation too:
                    // malformed reasoning/image metadata must not discard the
                    // account's otherwise valid transport template.
                    let official = entry.clone();
                    let mut entry = entry.clone();
                    capabilities.apply_to_codex(&mut entry);
                    entry["display_name"] = json!(runtime.codex_model_display_name(&upstream_id));
                    entry
                        .as_object()
                        .and_then(|normalized| {
                            normalize_native_codex_catalog_entry(
                                normalized,
                                &upstream_id,
                                priority,
                                None,
                            )
                        })
                        .map(|normalized| (normalized, official))
                })
            })
            .flatten();
        let mut model = native_catalog_model
            .as_ref()
            .map(|(normalized, _)| normalized.clone())
            .unwrap_or_else(|| routed_codex_catalog_entry(None, &display_id, priority, None));
        model["display_name"] = json!(runtime.codex_model_display_name(&upstream_id));
        // Account models and unqualified GPT IDs retain their public spelling,
        // including an explicitly configured key prefix. Qualified provider
        // IDs keep reversible aliases; a similar leaf is not the same model.
        // The GPT family rule changes only picker identity, never inventory,
        // route eligibility, or capability evidence.
        if has_native_account_route
            || (normalized.starts_with("gpt-") && !upstream_id.contains('/'))
        {
            model["slug"] = Value::String(display_id.clone());
        }
        for candidate_id in &native_account_ids {
            let uses_responses_lite = upstream_by_model
                .get(&normalized)
                .and_then(|entries| entries.iter().find(|(owner, _)| owner == candidate_id))
                .and_then(|(_, entry)| entry.get("use_responses_lite"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            runtime.set_codex_model_uses_responses_lite(
                candidate_id,
                &upstream_id,
                uses_responses_lite,
            );
        }
        capabilities.apply_to_codex(&mut model);
        if native_catalog_model.is_none() {
            crate::publish_routed_codex_context(&mut model, capabilities.context_limit);
        }
        let mut supported = runtime.client_reasoning_levels(key, &upstream_id, WireApi::Responses);
        let native_ultra = native_catalog_model.as_ref().is_some_and(|(_, official)| {
            apply_codex_ultra_from_official_model(&mut model, official, &upstream_id)
        });
        // Codex reads this live catalog, not the managed file. Use the same
        // installed official card, and only for that exact model.
        let installed_ultra = !native_ultra
            && runtime
                .official_codex_ultra_model(&upstream_id)
                .is_some_and(|official| {
                    apply_codex_ultra_from_official_model(&mut model, &official, &upstream_id)
                });
        if (native_ultra || installed_ultra) && !supported.iter().any(|level| level == "ultra") {
            supported.push("ultra".into());
        }
        let catalog_default = model["default_reasoning_level"]
            .as_str()
            .filter(|default| supported.iter().any(|level| level == default))
            .map(str::to_owned);
        apply_model_reasoning_allowed_levels(&mut model, Some(&supported));
        if let Some(default) = catalog_default {
            model["default_reasoning_level"] = json!(default);
        }
        if let Some(allowed) = runtime.model_reasoning_policy_levels(&upstream_id) {
            let allowed = allowed
                .into_iter()
                .filter(|level| supported.contains(level))
                .collect::<Vec<_>>();
            apply_model_reasoning_allowed_levels(&mut model, Some(&allowed));
        }
        // Speed follows the model family. Basis Points stays standard-only
        // and does not remove the picker.
        set_codex_service_tiers(
            &mut model,
            runtime.model_supported_service_tiers(&upstream_id),
        );
        sort_supported_reasoning_levels(&mut model);
        models.push(model);
    }

    normalize_codex_catalog_priorities(&mut models);
    if models.is_empty() {
        None
    } else {
        Some(json!({ "models": models }))
    }
}

pub(super) fn sort_supported_reasoning_levels(model: &mut Value) {
    let Some(levels) = model
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let order = crate::canonicalize_reasoning_levels(
        levels
            .iter()
            .filter_map(|level| level.get("effort").and_then(Value::as_str)),
    );
    levels.sort_by_key(|level| {
        let effort = level
            .get("effort")
            .and_then(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        order
            .iter()
            .position(|candidate| candidate == &effort)
            .unwrap_or(order.len())
    });
}

pub(super) fn apply_model_reasoning_allowed_levels(
    model: &mut Value,
    allowed_levels: Option<&[String]>,
) {
    let Some(allowed_levels) = allowed_levels else {
        // No override: preserve the projected reference modes.
        return;
    };
    if allowed_levels.is_empty() {
        model["supported_reasoning_levels"] = Value::Array(Vec::new());
        model["default_reasoning_summary"] = Value::String("none".into());
        model["supports_reasoning_summary_parameter"] = Value::Bool(false);
        model["supports_reasoning_summaries"] = Value::Bool(false);
        model
            .as_object_mut()
            .expect("normalized catalog entry is an object")
            .remove("default_reasoning_level");
        return;
    }
    let detected_levels = model
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let detected_by_effort = detected_levels
        .into_iter()
        .filter_map(|level| {
            let effort = level.get("effort")?.as_str()?.trim().to_ascii_lowercase();
            (!effort.is_empty()).then_some((effort, level))
        })
        .collect::<BTreeMap<_, _>>();
    let levels = crate::canonicalize_reasoning_levels(allowed_levels.iter())
        .into_iter()
        .map(|effort| {
            detected_by_effort
                .get(&effort)
                .cloned()
                .unwrap_or_else(|| json!({"effort": effort, "description": effort}))
        })
        .collect::<Vec<_>>();
    let (has_levels, default_reasoning_level) = {
        let default_reasoning_level = match levels.as_slice() {
            [] => None,
            [level] => level
                .get("effort")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => levels.iter().find_map(|level| {
                level
                    .get("effort")
                    .and_then(Value::as_str)
                    .filter(|effort| effort.eq_ignore_ascii_case("medium"))
                    .map(str::to_owned)
            }),
        };
        (!levels.is_empty(), default_reasoning_level)
    };
    model["supported_reasoning_levels"] = Value::Array(levels);
    if !has_levels {
        model["supported_reasoning_levels"] = Value::Array(Vec::new());
        model
            .as_object_mut()
            .expect("normalized catalog entry is an object")
            .remove("default_reasoning_level");
    } else if let Some(effort) = default_reasoning_level {
        model["default_reasoning_level"] = Value::String(effort);
    } else {
        model
            .as_object_mut()
            .expect("normalized catalog entry is an object")
            .remove("default_reasoning_level");
    }
    // A manually exposed effort does not prove a provider-specific summary
    // contract.  Keep the selector narrow and let the actual route decide.
    model["default_reasoning_summary"] = Value::String("none".into());
    model["supports_reasoning_summary_parameter"] = Value::Bool(false);
    model["supports_reasoning_summaries"] = Value::Bool(false);
}

pub(super) fn upstream_codex_models(payload: &Value) -> Option<&Vec<Value>> {
    payload
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| models.len() <= 4_096)
}

pub(super) fn allowed_openai_model_protocols(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
) -> Vec<WireApi> {
    let mut protocols = Vec::new();
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        protocols.push(WireApi::Responses);
    }
    if runtime.allows_client_wire_api(key, ClientWireApi::ChatCompletions) {
        protocols.push(WireApi::ChatCompletions);
    }
    protocols
}

pub(super) fn allowed_codex_model_protocols(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
) -> Vec<WireApi> {
    if runtime.allows_client_wire_api(key, ClientWireApi::Responses) {
        vec![WireApi::Responses]
    } else {
        Vec::new()
    }
}
