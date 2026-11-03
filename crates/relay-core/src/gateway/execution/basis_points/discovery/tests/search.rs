use super::*;

// Test-only reference of the original query ranking for valid synthetic inputs.
fn original_query<'a>(tools: &'a [ClientTool], query: &str, limit: usize) -> Vec<&'a ClientTool> {
    let terms = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let mut matches = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.key().to_lowercase();
            let text = format!("{} {}", name, tool.serialized_spec.as_ref()).to_lowercase();
            let score = terms
                .iter()
                .map(|term| usize::from(text.contains(term)) + 3 * usize::from(name.contains(term)))
                .sum::<usize>();
            (score > 0).then_some((score, tool))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.key().cmp(&right.key()))
    });
    matches
        .into_iter()
        .take(limit)
        .map(|(_, tool)| tool)
        .collect()
}

fn keys(tools: &[&ClientTool]) -> Vec<String> {
    tools.iter().map(|tool| tool.key()).collect()
}

fn synthetic_catalog(count: usize) -> Vec<ClientTool> {
    let tools = (0..count)
        .rev()
        .map(|index| {
            let mut tool = json!({
                "type":"function",
                "name":format!("synthetic_tool_{index:04}"),
                "description":"Synthetic usage rules. Чтение документа. ΟΣ İSTANBUL. ".repeat(16),
                "parameters":{
                    "type":"object",
                    "properties":{"payload":{"type":"string","minLength":1}},
                    "required":["payload"],
                    "additionalProperties":false
                }
            });
            if index % 3 == 0 {
                tool.as_object_mut().unwrap().remove("parameters");
                tool["type"] = json!("custom");
                tool["format"] =
                    json!({"type":"grammar","syntax":"lark","definition":"start: \"payload\""});
            }
            tool
        })
        .collect::<Vec<_>>();
    client_tools(&json!({"tools":[{"type":"namespace","name":"files","tools":tools}]})).unwrap()
}

#[test]
fn query_preserves_name_weight_case_sensitive_ties_and_limits() {
    let request = json!({"tools":[
        {"type":"function","name":"z_read","description":"read payload"},
        {"type":"function","name":"B_read","description":"read payload"},
        {"type":"function","name":"a_read","description":"read payload"},
        {"type":"function","name":"description_only","description":"read payload"},
        {"type":"namespace","name":"read","tools":[
            {"type":"custom","name":"action","description":"payload"}
        ]}
    ]});
    let tools = client_tools(&request).unwrap();
    let expected = [
        "B_read",
        "a_read",
        "read.action",
        "z_read",
        "description_only",
    ];
    for query in [
        "read",
        "READ",
        "read read",
        "read payload",
        "read___payload",
    ] {
        for limit in 1..=MAX_LOOKUP_TOOLS {
            let actual = lookup(&tools, &json!({"query":query,"limit":limit})).unwrap();
            assert_eq!(keys(&actual), expected[..limit.min(expected.len())]);
        }
        assert_eq!(
            keys(&lookup(&tools, &json!({"query":query})).unwrap()),
            expected
        );
    }
}

#[test]
fn query_matches_original_unicode_schema_search_and_stable_ranking() {
    for count in [0, 1, 9, 64] {
        let mut tools = synthetic_catalog(count);
        for _ in 0..3 {
            for query in [
                "synthetic_tool",
                "FILES.synthetic_tool_0003",
                "payload",
                "minLength",
                "ЧТЕНИЕ документа",
                "ΟΣ",
                "οσ",
                "İSTANBUL",
                "i̇stanbul",
                "read_read",
                "missing needle",
                "synthetic synthetic payload",
                "---...☃",
            ] {
                for limit in 1..=MAX_LOOKUP_TOOLS {
                    let actual = lookup(&tools, &json!({"query":query,"limit":limit})).unwrap();
                    let expected = original_query(&tools, query, limit);
                    assert_eq!(keys(&actual), keys(&expected), "{query:?}, limit={limit}");
                    for (actual, expected) in actual.into_iter().zip(expected) {
                        assert!(std::ptr::eq(actual, expected));
                    }
                }
            }
            if count > 1 {
                tools.rotate_left(count / 3 + 1);
            }
        }
    }
}

#[test]
fn query_without_search_terms_still_validates_arguments() {
    let tools = synthetic_catalog(1);
    assert!(lookup(&tools, &json!({"query":"---☃"})).unwrap().is_empty());
    for arguments in [
        json!({"query":"---☃","limit":0}),
        json!({"query":"---☃","limit":9}),
        json!({"query":"---☃","limit":1.5}),
        json!({"query":"---☃","unexpected":true}),
        json!({"query":" ","limit":1}),
        json!({"query":"-".repeat(501)}),
        json!({"query":"☃".repeat(167)}),
    ] {
        assert_eq!(
            lookup(&tools, &arguments).unwrap_err().parameter(),
            Some("output.relay.catalog.arguments")
        );
    }
}

#[test]
#[ignore = "paired synthetic catalog query measurement; not provider/request latency"]
fn measure_catalog_query_paired() {
    use sha2::{Digest, Sha256};
    use std::{hint::black_box, time::Instant};

    for count in [24, 256, 1024] {
        let tools = synthetic_catalog(count);
        let schema_bytes = tools
            .iter()
            .map(|tool| tool.serialized_spec.len())
            .sum::<usize>();
        for (label, query) in [
            ("name", "synthetic_tool"),
            ("schema", "payload"),
            ("unicode", "чтение документа"),
            ("missing", "absentneedle"),
        ] {
            let arguments = json!({"query":query,"limit":8});
            let expected = keys(&original_query(&tools, query, 8));
            assert_eq!(keys(&lookup(&tools, &arguments).unwrap()), expected);
            let measure = |current| {
                let started = Instant::now();
                for _ in 0..10 {
                    let tools = black_box(tools.as_slice());
                    if current {
                        black_box(lookup(tools, black_box(&arguments)).unwrap());
                    } else {
                        black_box(original_query(tools, black_box(query), 8));
                    }
                }
                started.elapsed()
            };
            let mut before = Vec::new();
            let mut after = Vec::new();
            for round in 0..9 {
                if round % 2 == 0 {
                    before.push(measure(false));
                    after.push(measure(true));
                } else {
                    after.push(measure(true));
                    before.push(measure(false));
                }
            }
            before.sort_unstable();
            after.sort_unstable();
            eprintln!(
                "catalog-query tools={count} schema_bytes={schema_bytes} query={label} iterations=10 reference_us={} current_us={} result_sha256={}",
                before[4].as_micros(),
                after[4].as_micros(),
                hex::encode(Sha256::digest(serde_json::to_vec(&expected).unwrap()))
            );
        }
    }
}
