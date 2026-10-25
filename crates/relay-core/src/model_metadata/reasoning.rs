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

fn select_reasoning_evidence<'a>(
    openrouter_record: Option<&'a ReasoningRecord>,
    litellm_record: Option<&'a ReasoningRecord>,
    models_dev_record: Option<&'a ReasoningRecord>,
) -> Option<(&'a ReasoningRecord, &'static str)> {
    let records = [
        ("openrouter", openrouter_record),
        ("litellm", litellm_record),
        ("models_dev", models_dev_record),
    ];

    records
        .into_iter()
        .find_map(|(source, record)| {
            record
                .filter(|reasoning_record| !reasoning_record.levels.is_empty())
                .map(|reasoning_record| (reasoning_record, source))
        })
        .or_else(|| {
            records.into_iter().find_map(|(source, record)| {
                record
                    .filter(|reasoning_record| reasoning_record.supported == Some(true))
                    .map(|reasoning_record| (reasoning_record, source))
            })
        })
}

fn first_reasoning_budget(
    budget_index: usize,
    records: [Option<&ReasoningRecord>; 3],
) -> Option<(u64, &'static str)> {
    records
        .into_iter()
        .zip(["openrouter", "litellm", "models_dev_details"])
        .find_map(|(record, source)| Some((record?.budget[budget_index]?, source)))
}

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
    enrich_reasoning_metadata_with_models_dev_details(
        models_dev.clone(),
        None,
        openrouter.cloned(),
        litellm.cloned(),
    )
}

pub(crate) fn enrich_reasoning_metadata_with_models_dev_details(
    models_dev: Value,
    models_dev_details: Option<Value>,
    openrouter: Option<Value>,
    litellm: Option<Value>,
) -> Value {
    // Extract compact reasoning evidence before consuming source records. It
    // must reflect each original source, not fields filled by the merge.
    let router: BTreeMap<String, ReasoningRecord> = openrouter
        .as_ref()
        .and_then(|openrouter_catalog| openrouter_catalog.get("data"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model_record| {
            let model_id = model_record.get("id")?.as_str()?;
            // Suffixes identify distinct catalog variants; never silently copy
            // their constraints onto the base model or collapse conflicting rows.
            Some((normalize(model_id), parse::openrouter_record(model_record)))
        })
        .collect();
    let lite: BTreeMap<String, ReasoningRecord> = litellm
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(id, model_record)| {
            id.as_str() != "sample_spec"
                && id.len() <= MAX_STRING_LENGTH
                && !id.trim().is_empty()
                && model_record.is_object()
        })
        .map(|(id, model_record)| (normalize(id), parse::litellm_record(model_record)))
        .collect();
    let detail_records = models_dev_details
        .as_ref()
        .map(parse::models_dev_details_records);
    let mut models = super::reference::merge_reference_records(
        models_dev,
        models_dev_details,
        openrouter,
        litellm,
    );
    let router = RecordIndex::new(&router);
    let lite = RecordIndex::new(&lite);
    let models_dev_details = detail_records.as_ref().map(RecordIndex::new);
    for (id, model_metadata) in &mut models {
        let Some(model_object) = model_metadata.as_object_mut() else {
            continue;
        };
        if model_object.get("reasoning").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let router_record = router.get(id);
        let lite_record = lite.get(id);
        let details_record = models_dev_details
            .as_ref()
            .and_then(|records| records.get(id));
        if let Some((reasoning_record, source)) =
            select_reasoning_evidence(router_record, lite_record, details_record)
        {
            let field_source = if source == "models_dev" {
                "models_dev_details"
            } else {
                source
            };
            super::provenance::set_field(
                model_object,
                "reasoning",
                Value::Bool(true),
                field_source,
            );
            model_object.insert("reasoning_source".into(), Value::String(source.into()));
            if !reasoning_record.levels.is_empty() {
                super::provenance::set_field(
                    model_object,
                    "reasoning_effort_levels",
                    serde_json::json!(reasoning_record.levels),
                    field_source,
                );
            }
            if let Some(method) = &reasoning_record.method {
                super::provenance::set_field(
                    model_object,
                    "reasoning_method",
                    serde_json::json!(method),
                    field_source,
                );
            }
            // Only publish defaults contained in the confirmed enum.
            super::provenance::remove_field(model_object, "default_reasoning_effort");
            if let Some(default) = reasoning_record
                .default
                .as_ref()
                .filter(|effort_level| reasoning_record.levels.contains(effort_level))
            {
                super::provenance::set_field(
                    model_object,
                    "default_reasoning_effort",
                    Value::String(default.clone()),
                    field_source,
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
                if let Some((budget, source)) =
                    first_reasoning_budget(index, [router_record, lite_record, details_record])
                {
                    super::provenance::set_field(model_object, key, budget.into(), source);
                }
            }
        }
    }
    // Move the merged records into the JSON object; serializing the map here
    // would allocate a second complete tree while the first is still alive.
    Value::Object(models.into_iter().collect())
}
