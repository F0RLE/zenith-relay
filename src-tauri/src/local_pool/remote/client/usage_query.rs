use zenith_relay_core::protocol::{UsageQuery, UsageRange};

pub(super) fn usage_path(query: &UsageQuery) -> String {
    let mut parameters = url::form_urlencoded::Serializer::new(String::new());
    let (page, page_size) = query.normalized_page();
    parameters.append_pair("page", &page.to_string());
    parameters.append_pair("pageSize", &page_size.to_string());
    if let Some(usage_range) = query.range {
        parameters.append_pair("range", usage_range_name(usage_range));
    }
    append_number(&mut parameters, "fromMs", query.from_ms);
    append_number(&mut parameters, "toMs", query.to_ms);
    append_number(&mut parameters, "bucketMs", query.bucket_ms);
    append_text(&mut parameters, "modelQuery", query.model_query.as_deref());
    append_text(
        &mut parameters,
        "sourceOrAccountQuery",
        query.source_or_account_query.as_deref(),
    );
    if let Some(wire_api) = query.wire_api {
        parameters.append_pair("wireApi", wire_api.as_str());
    }
    if let Some(success) = query.success {
        parameters.append_pair("success", if success { "true" } else { "false" });
    }
    append_text(
        &mut parameters,
        "errorCategory",
        query.error_category.as_deref(),
    );
    append_text(
        &mut parameters,
        "requestIdQuery",
        query.request_id_query.as_deref(),
    );
    if let Some(include_events) = query.include_events {
        parameters.append_pair(
            "includeEvents",
            if include_events { "true" } else { "false" },
        );
    }
    if let Some(include_models) = query.include_models {
        parameters.append_pair(
            "includeModels",
            if include_models { "true" } else { "false" },
        );
    }
    if let Some(include_pool_members) = query.include_pool_members {
        parameters.append_pair(
            "includePoolMembers",
            if include_pool_members {
                "true"
            } else {
                "false"
            },
        );
    }
    format!("/usage?{}", parameters.finish())
}

fn append_text(
    parameters: &mut url::form_urlencoded::Serializer<'_, String>,
    parameter_name: &str,
    text_value: Option<&str>,
) {
    if let Some(text_value) = text_value.filter(|text| !text.is_empty()) {
        parameters.append_pair(parameter_name, text_value);
    }
}

fn append_number(
    parameters: &mut url::form_urlencoded::Serializer<'_, String>,
    parameter_name: &str,
    numeric_value: Option<u64>,
) {
    if let Some(numeric_value) = numeric_value {
        parameters.append_pair(parameter_name, &numeric_value.to_string());
    }
}

fn usage_range_name(range: UsageRange) -> &'static str {
    match range {
        UsageRange::Daily => "daily",
        UsageRange::Weekly => "weekly",
        UsageRange::Monthly => "monthly",
        UsageRange::Custom => "custom",
    }
}
