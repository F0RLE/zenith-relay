use super::{model_leaf, normalize, validate_payload, ReasoningMethod, MAX_STRING_LENGTH};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub(super) struct ReasoningRecord {
    pub(super) supported: Option<bool>,
    pub(super) method: Option<ReasoningMethod>,
    pub(super) levels: Vec<String>,
    pub(super) default: Option<String>,
    pub(super) budget: [Option<u64>; 3],
}

pub(super) fn normalize_external_levels<I, S>(levels: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    crate::canonicalize_reasoning_levels(levels.into_iter().filter_map(|level| {
        let value = level.as_ref().trim().to_ascii_lowercase();
        matches!(
            value.as_str(),
            "none"
                | "minimal"
                | "low"
                | "medium"
                | "high"
                | "xhigh"
                | "very_high"
                | "extra_high"
                | "max"
                | "ultra"
        )
        .then_some(value)
    }))
}

fn level_values(value: &Value) -> Vec<String> {
    let value = value
        .get("values")
        .or_else(|| value.get("enum"))
        .unwrap_or(value);
    if let Some(values) = value.as_array() {
        normalize_external_levels(values.iter().filter_map(Value::as_str))
    } else {
        normalize_external_levels(value.as_str())
    }
}

pub(super) fn parse_effort_flags(object: &Map<String, Value>) -> Vec<String> {
    normalize_external_levels(object.iter().filter_map(|(key, value)| {
        if value.as_bool() != Some(true) {
            return None;
        }
        let key = key.replace('_', "").to_ascii_lowercase();
        key.strip_prefix("supports")?
            .strip_suffix("reasoningeffort")
            .map(str::to_string)
    }))
}

pub(super) fn parse_reasoning_object(value: Option<&Value>) -> ReasoningRecord {
    let Some(object) = value.and_then(Value::as_object) else {
        return ReasoningRecord {
            supported: value.and_then(Value::as_bool),
            ..ReasoningRecord::default()
        };
    };
    let mut record = ReasoningRecord {
        supported: object.get("supported").and_then(Value::as_bool),
        ..ReasoningRecord::default()
    };
    record.levels = normalize_external_levels(
        [
            "supported_efforts",
            "supportedEfforts",
            "efforts",
            "effort",
            "values",
        ]
        .into_iter()
        .filter_map(|key| object.get(key))
        .flat_map(level_values),
    );
    record.default = object
        .get("default_effort")
        .or_else(|| object.get("defaultEffort"))
        .and_then(Value::as_str)
        .and_then(|value| normalize_external_levels([value]).pop());
    let budget_object = object
        .get("budget_tokens")
        .or_else(|| object.get("budgetTokens"));
    let number = |keys: &[&str], nested_key: &str| {
        keys.iter()
            .find_map(|key| object.get(*key).and_then(Value::as_u64))
            .or_else(|| {
                budget_object
                    .and_then(|v| v.get(nested_key))
                    .and_then(Value::as_u64)
            })
    };
    record.budget = [
        number(&["min_budget_tokens", "minBudgetTokens"], "min"),
        number(&["max_budget_tokens", "maxBudgetTokens"], "max"),
        number(
            &[
                "default_budget_tokens",
                "defaultBudgetTokens",
                "budget_tokens",
                "budgetTokens",
            ],
            "default",
        ),
    ];
    if matches!((record.budget[0], record.budget[1]), (Some(min), Some(max)) if min > max) {
        record.budget = [None; 3];
    }
    if record.budget[2].is_some_and(|v| {
        record.budget[0].is_some_and(|min| v < min) || record.budget[1].is_some_and(|max| v > max)
    }) {
        record.budget[2] = None;
    }
    record.method = object
        .get("type")
        .and_then(Value::as_str)
        .map(|kind| match kind {
            "effort" => ReasoningMethod::Effort,
            "toggle" => ReasoningMethod::Toggle,
            "budget_tokens" | "budgetTokens" => ReasoningMethod::BudgetTokens,
            "adaptive" | "adaptive_effort" | "adaptiveEffort" => ReasoningMethod::Adaptive,
            _ => ReasoningMethod::Unknown,
        })
        .or_else(|| {
            if !record.levels.is_empty() {
                Some(ReasoningMethod::Effort)
            } else if budget_object.is_some() || record.budget.iter().any(Option::is_some) {
                Some(ReasoningMethod::BudgetTokens)
            } else if object.contains_key("enabled") || object.contains_key("default_enabled") {
                Some(ReasoningMethod::Toggle)
            } else {
                None
            }
        });
    if !record.levels.is_empty() || record.method.is_some() {
        record.supported = Some(true);
    }
    record
}

/// Models.dev represents reasoning controls as an array of option objects,
/// while OpenRouter commonly uses one reasoning object. Normalize the array
/// form before the shared metadata parser consumes it.
pub(super) fn parse_reasoning_options(value: Option<&Value>) -> ReasoningRecord {
    let Some(options) = value.and_then(Value::as_array) else {
        return ReasoningRecord::default();
    };
    let mut record = ReasoningRecord::default();
    for option in options {
        let parsed = parse_reasoning_object(Some(option));
        record.supported = parsed.supported.or(record.supported);
        record.method = record.method.or(parsed.method);
        record.levels.extend(parsed.levels);
        record.default = record.default.or(parsed.default);
        for (index, budget) in parsed.budget.into_iter().enumerate() {
            record.budget[index] = record.budget[index].or(budget);
        }
    }
    record.levels = normalize_external_levels(record.levels);
    if !record.levels.is_empty() || record.method.is_some() {
        record.supported = Some(true);
    }
    record
}

fn openrouter_record(value: &Value) -> ReasoningRecord {
    let mut record = parse_reasoning_object(value.get("reasoning"));
    let params = value
        .get("supported_parameters")
        .or_else(|| value.get("supportedParameters"))
        .and_then(Value::as_array);
    let has = |param: &str| params.is_some_and(|v| v.iter().any(|p| p.as_str() == Some(param)));
    if has("reasoning_effort") {
        record.supported = Some(true);
        record.method.get_or_insert(ReasoningMethod::Effort);
    } else if has("reasoning") {
        record.supported = Some(true);
        record.method.get_or_insert(ReasoningMethod::Unknown);
    }
    record
}

fn litellm_record(value: &Value) -> ReasoningRecord {
    let mut record = parse_reasoning_object(value.get("reasoning"));
    if let Some(object) = value.as_object() {
        record.levels = parse_effort_flags(object);
        record.supported = object
            .get("supports_reasoning")
            .or_else(|| object.get("supportsReasoning"))
            .and_then(Value::as_bool)
            .or(record.supported);
        if !record.levels.is_empty() {
            record.supported = Some(true);
            record.method = Some(ReasoningMethod::Effort);
        }
    }
    record
}

fn models_dev_details_records(value: &Value) -> BTreeMap<String, ReasoningRecord> {
    let mut records = BTreeMap::new();
    let Some(providers) = value.as_object() else {
        return records;
    };
    for (provider, provider_value) in providers {
        let Some(models) = provider_value.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (key, model) in models {
            let Some(object) = model.as_object() else {
                continue;
            };
            let model_id = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .unwrap_or(key);
            let qualified_id = if model_id.contains('/') {
                model_id.to_string()
            } else {
                format!("{provider}/{model_id}")
            };
            let mut record = parse_reasoning_object(object.get("reasoning"));
            let options = parse_reasoning_options(
                object
                    .get("reasoning_options")
                    .or_else(|| object.get("reasoningOptions")),
            );
            record.supported = options.supported.or(record.supported);
            record.method = options.method.or(record.method);
            // Details payloads can expose a broad `reasoning` enum alongside
            // a separate control such as a budget toggle in
            // `reasoning_options`. Keep both sources: replacing the enum with
            // an empty options list silently hid supported effort levels.
            record.levels =
                normalize_external_levels(record.levels.into_iter().chain(options.levels));
            record.default = options.default.or(record.default);
            for (index, budget) in options.budget.into_iter().enumerate() {
                record.budget[index] = record.budget[index].or(budget);
            }
            records.insert(normalize(&qualified_id), record);
        }
    }
    records
}

fn base_id(id: &str) -> &str {
    id.split_once(':').map_or(id, |(base, _)| base)
}

/// Models.dev spells decimal release components with a dash while other
/// registries commonly keep the decimal point (`claude-opus-4-8` versus
/// `claude-opus-4.8`). Normalize only separators surrounded by ASCII digits;
/// arbitrary punctuation and provider/model boundaries remain significant.
fn version_match_id(id: &str) -> String {
    let id = base_id(id);
    let bytes = id.as_bytes();
    let mut normalized = String::with_capacity(id.len());
    for (index, character) in id.char_indices() {
        if character == '.'
            && index > 0
            && index + 1 < bytes.len()
            && bytes[index - 1].is_ascii_digit()
            && bytes[index + 1].is_ascii_digit()
        {
            normalized.push('-');
        } else {
            normalized.push(character);
        }
    }
    normalized
}

// Provider-qualified IDs match their exact external ID or a suffixed variant
// (OpenRouter uses `:free`, `:nitro`, etc.). An unqualified leaf is accepted
// only when it identifies one external record; this prevents cross-provider
// capability leakage.
fn matching_record<'a, T>(records: &'a BTreeMap<String, T>, id: &str) -> Option<&'a T> {
    let key = normalize(id);
    if let Some(value) = records.get(&key) {
        return Some(value);
    }
    let version_key = version_match_id(&key);
    let provider_prefix = key
        .split_once('/')
        .map(|(provider, _)| format!("{provider}/"));

    // Prefer the unsuffixed registry row. Variant rows such as `:batch` may
    // coexist with it and must not make an otherwise exact version match
    // ambiguous.
    let mut unsuffixed = records.iter().filter(|(candidate, _)| {
        !candidate.contains(':')
            && version_match_id(candidate) == version_key
            && (!key.contains('/')
                || provider_prefix
                    .as_ref()
                    .is_some_and(|prefix| candidate.starts_with(prefix)))
    });
    if let Some((_, value)) = unsuffixed.next() {
        if unsuffixed.next().is_none() {
            return Some(value);
        }
        return None;
    }
    let mut candidates = records.iter().filter(|(candidate, _)| {
        version_match_id(candidate) == version_key
            && (!key.contains('/')
                || provider_prefix
                    .as_ref()
                    .is_some_and(|prefix| candidate.starts_with(prefix)))
    });
    if let Some((_, value)) = candidates.next() {
        return candidates.next().is_none().then_some(value);
    }
    let leaf = model_leaf(&key);
    let mut candidates = records.iter().filter(|(candidate, _)| {
        model_leaf(candidate) == leaf
            && (!key.contains('/')
                || !candidate.contains('/')
                || provider_prefix
                    .as_ref()
                    .is_some_and(|prefix| candidate.starts_with(prefix)))
    });
    let (_, value) = candidates.next()?;
    candidates.next().is_none().then_some(value)
}

#[cfg(test)]
pub(crate) fn enrich_reasoning_metadata(
    models_dev: &Value,
    openrouter: Option<&Value>,
    litellm: Option<&Value>,
) -> Value {
    enrich_reasoning_metadata_with_models_dev_details(models_dev, None, openrouter, litellm)
}

pub(crate) fn enrich_reasoning_metadata_with_models_dev_details(
    models_dev: &Value,
    models_dev_details: Option<&Value>,
    openrouter: Option<&Value>,
    litellm: Option<&Value>,
) -> Value {
    let mut models: BTreeMap<String, Value> = validate_payload(models_dev)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, value)| (normalize(&id), value.clone()))
        .collect();
    let router: BTreeMap<String, ReasoningRecord> = openrouter
        .and_then(|v| v.get("data"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let id = value.get("id")?.as_str()?;
            // Suffixes identify distinct catalog variants; never silently copy
            // their constraints onto the base model or collapse conflicting rows.
            Some((normalize(id), openrouter_record(value)))
        })
        .collect();
    let lite: BTreeMap<String, ReasoningRecord> = litellm
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(id, v)| {
            id.as_str() != "sample_spec"
                && id.len() <= MAX_STRING_LENGTH
                && !id.trim().is_empty()
                && v.is_object()
        })
        .map(|(id, value)| (normalize(id), litellm_record(value)))
        .collect();
    let models_dev_details = models_dev_details.map(models_dev_details_records);
    for (id, value) in &mut models {
        let Some(object) = value.as_object_mut() else {
            continue;
        };
        let router_record = matching_record(&router, id);
        let lite_record = matching_record(&lite, id);
        let details_record = models_dev_details
            .as_ref()
            .and_then(|records| matching_record(records, id));
        let exact = router_record
            .filter(|r| !r.levels.is_empty())
            .map(|r| (r, "openrouter"))
            .or_else(|| {
                lite_record
                    .filter(|r| !r.levels.is_empty())
                    .map(|r| (r, "litellm"))
            })
            .or_else(|| {
                details_record
                    .filter(|r| !r.levels.is_empty())
                    .map(|r| (r, "models_dev"))
            });
        let evidence = exact
            .or_else(|| {
                router_record
                    .filter(|r| r.supported == Some(true))
                    .map(|r| (r, "openrouter"))
            })
            .or_else(|| {
                lite_record
                    .filter(|r| r.supported == Some(true))
                    .map(|r| (r, "litellm"))
            })
            .or_else(|| {
                details_record
                    .filter(|r| r.supported == Some(true))
                    .map(|r| (r, "models_dev"))
            });
        if let Some((record, source)) = evidence {
            object.insert("reasoning".into(), Value::Bool(true));
            object.insert("reasoning_source".into(), Value::String(source.into()));
            if !record.levels.is_empty() {
                object.insert(
                    "reasoning_effort_levels".into(),
                    serde_json::json!(record.levels),
                );
            }
            if let Some(method) = &record.method {
                object.insert("reasoning_method".into(), serde_json::json!(method));
            }
            // Only publish defaults contained in the confirmed enum.
            object.remove("default_reasoning_effort");
            if let Some(default) = record
                .default
                .as_ref()
                .filter(|v| record.levels.contains(v))
            {
                object.insert(
                    "default_reasoning_effort".into(),
                    Value::String(default.clone()),
                );
            }
        } else if object.get("reasoning").and_then(Value::as_bool).is_some() {
            object.insert(
                "reasoning_source".into(),
                Value::String("models_dev".into()),
            );
        }
        if router_record.is_some() || lite_record.is_some() || details_record.is_some() {
            for (index, key) in [
                "reasoning_budget_min_tokens",
                "reasoning_budget_max_tokens",
                "reasoning_budget_default_tokens",
            ]
            .into_iter()
            .enumerate()
            {
                let budget = router_record
                    .and_then(|router| router.budget[index])
                    .or_else(|| lite_record.and_then(|lite| lite.budget[index]));
                let budget =
                    budget.or_else(|| details_record.and_then(|details| details.budget[index]));
                if let Some(budget) = budget {
                    object.insert(key.to_string(), budget.into());
                }
            }
        }
    }
    serde_json::to_value(models).unwrap_or_default()
}
