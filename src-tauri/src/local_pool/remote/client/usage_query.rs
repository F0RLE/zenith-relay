use zenith_relay_core::protocol::{UsageQuery, UsageRange};

pub(super) fn usage_path(query: &UsageQuery) -> String {
    let mut parameters = url::form_urlencoded::Serializer::new(String::new());
    let (page, page_size) = query.normalized_page();
    parameters.append_pair("page", &page.to_string());
    parameters.append_pair("pageSize", &page_size.to_string());
    if let Some(value) = query.range {
        parameters.append_pair("range", usage_range_name(value));
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
    if let Some(value) = query.wire_api {
        parameters.append_pair("wireApi", value.as_str());
    }
    if let Some(value) = query.success {
        parameters.append_pair("success", if value { "true" } else { "false" });
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
    if let Some(value) = query.include_events {
        parameters.append_pair("includeEvents", if value { "true" } else { "false" });
    }
    if let Some(value) = query.include_models {
        parameters.append_pair("includeModels", if value { "true" } else { "false" });
    }
    if let Some(value) = query.include_pool_members {
        parameters.append_pair("includePoolMembers", if value { "true" } else { "false" });
    }
    format!("/usage?{}", parameters.finish())
}

fn append_text(
    parameters: &mut url::form_urlencoded::Serializer<'_, String>,
    name: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        parameters.append_pair(name, value);
    }
}

fn append_number(
    parameters: &mut url::form_urlencoded::Serializer<'_, String>,
    name: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        parameters.append_pair(name, &value.to_string());
    }
}

fn usage_range_name(value: UsageRange) -> &'static str {
    match value {
        UsageRange::Daily => "daily",
        UsageRange::Weekly => "weekly",
        UsageRange::Monthly => "monthly",
        UsageRange::Custom => "custom",
    }
}
