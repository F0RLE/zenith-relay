use super::super::ReasoningMethod;
use super::super::order::normalize;
use super::ReasoningRecord;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub(in crate::model_metadata) fn normalize_external_levels<I, S>(levels: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    crate::canonicalize_reasoning_levels(levels.into_iter().filter_map(|level| {
        let normalized_level = crate::catalog::canonical_reasoning_level(level.as_ref());
        matches!(
            normalized_level.as_str(),
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
        )
        .then_some(normalized_level)
    }))
}

fn level_values(level_definition: &Value) -> Vec<String> {
    let level_values = level_definition
        .get("values")
        .or_else(|| level_definition.get("enum"))
        .unwrap_or(level_definition);
    if let Some(values) = level_values.as_array() {
        normalize_external_levels(values.iter().filter_map(Value::as_str))
    } else {
        normalize_external_levels(level_values.as_str())
    }
}

fn is_level_enum(level_definition: &Value) -> bool {
    level_definition
        .get("values")
        .or_else(|| level_definition.get("enum"))
        .unwrap_or(level_definition)
        .is_array()
}

fn merge_levels(record: &mut ReasoningRecord, levels: Vec<String>, explicit: bool) {
    if record.levels_explicit && !explicit {
        return;
    }
    if explicit && !record.levels_explicit {
        record.levels = levels;
    } else {
        record.levels.extend(levels);
    }
    record.levels_explicit |= explicit;
    record.levels = normalize_external_levels(std::mem::take(&mut record.levels));
}

pub(in crate::model_metadata) fn parse_effort_flags(
    model_object: &Map<String, Value>,
) -> Vec<String> {
    normalize_external_levels(model_object.iter().filter_map(|(key, flag_value)| {
        if flag_value.as_bool() != Some(true) {
            return None;
        }
        let key = key.replace('_', "").to_ascii_lowercase();
        key.strip_prefix("supports")?
            .strip_suffix("reasoningeffort")
            .map(str::to_string)
    }))
}

pub(in crate::model_metadata) fn parse_reasoning_object(
    reasoning_value: Option<&Value>,
) -> ReasoningRecord {
    let Some(reasoning_object) = reasoning_value.and_then(Value::as_object) else {
        return ReasoningRecord {
            supported: reasoning_value.and_then(Value::as_bool),
            ..ReasoningRecord::default()
        };
    };
    let mut reasoning_record = ReasoningRecord {
        supported: reasoning_object.get("supported").and_then(Value::as_bool),
        ..ReasoningRecord::default()
    };
    let definitions: Vec<_> = [
        "supported_efforts",
        "supportedEfforts",
        "efforts",
        "effort",
        "values",
    ]
    .into_iter()
    .filter_map(|key| reasoning_object.get(key))
    .collect();
    reasoning_record.levels = normalize_external_levels(
        definitions
            .iter()
            .filter(|value| is_level_enum(value))
            .flat_map(|value| level_values(value)),
    );
    reasoning_record.levels_explicit = !reasoning_record.levels.is_empty();
    if !reasoning_record.levels_explicit {
        reasoning_record.levels =
            normalize_external_levels(definitions.into_iter().flat_map(level_values));
    }
    reasoning_record.default = reasoning_object
        .get("default_effort")
        .or_else(|| reasoning_object.get("defaultEffort"))
        .and_then(Value::as_str)
        .and_then(|level| normalize_external_levels([level]).pop());
    let budget_object = reasoning_object
        .get("budget_tokens")
        .or_else(|| reasoning_object.get("budgetTokens"));
    let budget_number = |keys: &[&str], nested_key: &str| {
        keys.iter()
            .find_map(|key| reasoning_object.get(*key).and_then(Value::as_u64))
            .or_else(|| reasoning_object.get(nested_key).and_then(Value::as_u64))
            .or_else(|| {
                budget_object
                    .and_then(|budget_value| budget_value.get(nested_key))
                    .and_then(Value::as_u64)
            })
    };
    reasoning_record.budget = [
        budget_number(&["min_budget_tokens", "minBudgetTokens"], "min"),
        budget_number(&["max_budget_tokens", "maxBudgetTokens"], "max"),
        budget_number(
            &[
                "default_budget_tokens",
                "defaultBudgetTokens",
                "budget_tokens",
                "budgetTokens",
            ],
            "default",
        ),
    ];
    if matches!((reasoning_record.budget[0], reasoning_record.budget[1]), (Some(min), Some(max)) if min > max)
    {
        reasoning_record.budget = [None; 3];
    }
    if reasoning_record.budget[2].is_some_and(|default_budget| {
        reasoning_record.budget[0].is_some_and(|min_budget| default_budget < min_budget)
            || reasoning_record.budget[1].is_some_and(|max_budget| default_budget > max_budget)
    }) {
        reasoning_record.budget[2] = None;
    }
    reasoning_record.method = reasoning_object
        .get("type")
        .and_then(Value::as_str)
        .map(|reasoning_type| match reasoning_type {
            "effort" => ReasoningMethod::Effort,
            "toggle" => ReasoningMethod::Toggle,
            "budget_tokens" | "budgetTokens" => ReasoningMethod::BudgetTokens,
            "adaptive" | "adaptive_effort" | "adaptiveEffort" => ReasoningMethod::Adaptive,
            _ => ReasoningMethod::Unknown,
        })
        .or_else(|| {
            if !reasoning_record.levels.is_empty() {
                Some(ReasoningMethod::Effort)
            } else if budget_object.is_some() || reasoning_record.budget.iter().any(Option::is_some)
            {
                Some(ReasoningMethod::BudgetTokens)
            } else if reasoning_object.contains_key("enabled")
                || reasoning_object.contains_key("default_enabled")
            {
                Some(ReasoningMethod::Toggle)
            } else {
                None
            }
        });
    if reasoning_record.supported != Some(false)
        && (!reasoning_record.levels.is_empty() || reasoning_record.method.is_some())
    {
        reasoning_record.supported = Some(true);
    }
    reasoning_record
}

/// Models.dev represents reasoning controls as an array of option objects,
/// while OpenRouter commonly uses one reasoning object. Normalize the array
/// form before the shared metadata parser consumes it.
pub(in crate::model_metadata) fn parse_reasoning_options(
    reasoning_options_value: Option<&Value>,
) -> ReasoningRecord {
    let Some(options) = reasoning_options_value.and_then(Value::as_array) else {
        return ReasoningRecord::default();
    };
    let mut reasoning_record = ReasoningRecord::default();
    for option in options {
        let parsed = parse_reasoning_object(Some(option));
        reasoning_record.supported = parsed.supported.or(reasoning_record.supported);
        reasoning_record.method = reasoning_record.method.or(parsed.method);
        merge_levels(&mut reasoning_record, parsed.levels, parsed.levels_explicit);
        reasoning_record.default = reasoning_record.default.or(parsed.default);
        for (index, budget) in parsed.budget.into_iter().enumerate() {
            reasoning_record.budget[index] = reasoning_record.budget[index].or(budget);
        }
    }
    reasoning_record.levels = normalize_external_levels(reasoning_record.levels);
    if reasoning_record.supported != Some(false)
        && (!reasoning_record.levels.is_empty() || reasoning_record.method.is_some())
    {
        reasoning_record.supported = Some(true);
    }
    reasoning_record
}

pub(super) fn openrouter_record(model_record: &Value) -> ReasoningRecord {
    let mut reasoning_record = parse_model_reasoning(model_record);
    if reasoning_record.supported == Some(false) {
        return reasoning_record;
    }
    let supported_parameters = model_record
        .get("supported_parameters")
        .or_else(|| model_record.get("supportedParameters"))
        .and_then(Value::as_array);
    let supports_parameter = |parameter: &str| {
        supported_parameters.is_some_and(|parameters| {
            parameters
                .iter()
                .any(|parameter_name| parameter_name.as_str() == Some(parameter))
        })
    };
    if supports_parameter("reasoning_effort") {
        reasoning_record.supported = Some(true);
        reasoning_record
            .method
            .get_or_insert(ReasoningMethod::Effort);
    } else if supports_parameter("reasoning") {
        reasoning_record.supported = Some(true);
        reasoning_record
            .method
            .get_or_insert(ReasoningMethod::Unknown);
    }
    reasoning_record
}

pub(super) fn litellm_record(model_record: &Value) -> ReasoningRecord {
    parse_model_reasoning(model_record)
}

/// Keep declared effort enums distinct from individual positive feature flags
/// until source selection is complete. A scalar effort is not an exhaustive enum.
pub(in crate::model_metadata) fn parse_model_reasoning(model_record: &Value) -> ReasoningRecord {
    let mut reasoning_record = parse_reasoning_object(model_record.get("reasoning"));
    if let Some(model_object) = model_record.as_object() {
        let options = parse_reasoning_options(
            model_object
                .get("reasoning_options")
                .or_else(|| model_object.get("reasoningOptions")),
        );
        reasoning_record.supported = reasoning_record.supported.or(options.supported);
        reasoning_record.method = options.method.or(reasoning_record.method);
        merge_levels(
            &mut reasoning_record,
            options.levels,
            options.levels_explicit,
        );
        reasoning_record.default = options.default.or(reasoning_record.default);
        for (index, budget) in options.budget.into_iter().enumerate() {
            reasoning_record.budget[index] = reasoning_record.budget[index].or(budget);
        }
        if let Some(definition) = model_object
            .get("reasoning_effort_levels")
            .or_else(|| model_object.get("reasoningEffortLevels"))
        {
            let levels = level_values(definition);
            if !levels.is_empty()
                && (is_level_enum(definition) || !reasoning_record.levels_explicit)
            {
                reasoning_record.levels = levels;
                reasoning_record.levels_explicit = is_level_enum(definition);
            }
        }
        reasoning_record.method = model_object
            .get("reasoning_method")
            .and_then(|method| serde_json::from_value(method.clone()).ok())
            .or(reasoning_record.method);
        reasoning_record.default = model_object
            .get("default_reasoning_effort")
            .or_else(|| model_object.get("defaultReasoningEffort"))
            .and_then(Value::as_str)
            .and_then(|level| normalize_external_levels([level]).pop())
            .or(reasoning_record.default);
        if !reasoning_record.levels_explicit {
            let flags = parse_effort_flags(model_object);
            merge_levels(&mut reasoning_record, flags, false);
        }
        if reasoning_record.supported != Some(false) {
            reasoning_record.supported = model_object
                .get("supports_reasoning")
                .or_else(|| model_object.get("supportsReasoning"))
                .and_then(Value::as_bool)
                .or(reasoning_record.supported);
        }
        if reasoning_record.supported != Some(false) && !reasoning_record.levels.is_empty() {
            reasoning_record.supported = Some(true);
            reasoning_record
                .method
                .get_or_insert(ReasoningMethod::Effort);
        }
    }
    reasoning_record
}

pub(super) fn models_dev_details_records(
    catalog_payload: &Value,
) -> BTreeMap<String, ReasoningRecord> {
    let mut reasoning_records = BTreeMap::new();
    let Some(providers) = catalog_payload.as_object() else {
        return reasoning_records;
    };
    for (provider, provider_record) in providers {
        let Some(models) = provider_record.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_key, model_value) in models {
            let Some(model_object) = model_value.as_object() else {
                continue;
            };
            let model_id = model_object
                .get("id")
                .and_then(Value::as_str)
                .filter(|model_id| !model_id.trim().is_empty())
                .unwrap_or(model_key);
            let qualified_id = super::super::reference::provider_scoped_id(provider, model_id);
            let mut reasoning_record = parse_model_reasoning(model_value);
            reasoning_record.source_model_id = qualified_id.clone();
            reasoning_records.insert(normalize(&qualified_id), reasoning_record);
        }
    }
    reasoning_records
}
