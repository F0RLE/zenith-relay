use super::order::normalize;
use super::{ReasoningMethod, MAX_STRING_LENGTH};
use serde_json::Value;
use std::collections::BTreeMap;

mod matching;
mod parse;

use matching::RecordIndex;
pub(super) use parse::{
    normalize_external_levels, parse_effort_flags, parse_reasoning_object, parse_reasoning_options,
};

#[derive(Clone, Debug, Default)]
pub(super) struct ReasoningRecord {
    pub(super) supported: Option<bool>,
    pub(super) method: Option<ReasoningMethod>,
    pub(super) levels: Vec<String>,
    pub(super) default: Option<String>,
    pub(super) budget: [Option<u64>; 3],
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
    let mut models = super::reference::merge_reference_records(
        models_dev,
        models_dev_details,
        openrouter,
        litellm,
    );
    let router: BTreeMap<String, ReasoningRecord> = openrouter
        .and_then(|v| v.get("data"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let id = value.get("id")?.as_str()?;
            // Suffixes identify distinct catalog variants; never silently copy
            // their constraints onto the base model or collapse conflicting rows.
            Some((normalize(id), parse::openrouter_record(value)))
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
        .map(|(id, value)| (normalize(id), parse::litellm_record(value)))
        .collect();
    let models_dev_details = models_dev_details.map(parse::models_dev_details_records);
    let router = RecordIndex::new(&router);
    let lite = RecordIndex::new(&lite);
    let models_dev_details = models_dev_details.as_ref().map(RecordIndex::new);
    for (id, value) in &mut models {
        let Some(object) = value.as_object_mut() else {
            continue;
        };
        if object.get("reasoning").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let router_record = router.get(id);
        let lite_record = lite.get(id);
        let details_record = models_dev_details
            .as_ref()
            .and_then(|records| records.get(id));
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
    // Move the merged records into the JSON object; serializing the map here
    // would allocate a second complete tree while the first is still alive.
    Value::Object(models.into_iter().collect())
}
