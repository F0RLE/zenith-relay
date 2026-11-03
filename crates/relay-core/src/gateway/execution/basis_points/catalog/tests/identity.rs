use super::*;

// Test-only pre-optimization references; keep their allocating key comparisons.
fn original_choice_matches(choice: &Value, tool: &ClientTool) -> bool {
    let Some(name) = choice.get("name").and_then(Value::as_str) else {
        return false;
    };
    let matches = match choice.get("namespace").and_then(Value::as_str) {
        Some(namespace) => tool.namespace.as_deref() == Some(namespace) && tool.name == name,
        None => name == tool.name || name == tool.key(),
    };
    matches
        && choice
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == tool.kind)
}

fn original_selected_tools(request: &Value, tools: &[ClientTool]) -> Vec<ClientTool> {
    let Some(choice) = request.get("tool_choice") else {
        return tools.to_vec();
    };
    if choice.as_str() == Some("none") {
        return Vec::new();
    }
    let Some(object) = choice.as_object() else {
        return tools.to_vec();
    };
    if object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let Some(allowed) = object.get("tools").and_then(Value::as_array) else {
            return Vec::new();
        };
        return tools
            .iter()
            .filter(|tool| {
                allowed
                    .iter()
                    .any(|choice| original_choice_matches(choice, tool))
            })
            .cloned()
            .collect();
    }
    if object.get("type").and_then(Value::as_str) == Some("auto") {
        return tools.to_vec();
    }
    tools
        .iter()
        .filter(|tool| original_choice_matches(choice, tool))
        .cloned()
        .collect()
}

fn original_tool_spec<'a>(tools: &'a [ClientTool], name: &str) -> Option<&'a ClientTool> {
    if let Some(tool) = tools.iter().find(|tool| tool.key() == name) {
        return Some(tool);
    }
    let mut matches = tools.iter().filter(|tool| tool.name == name);
    let tool = matches.next()?;
    matches.next().is_none().then_some(tool)
}

fn identity_catalog() -> Vec<ClientTool> {
    client_tools(&json!({"tools":[
        {"type":"namespace","name":"files","tools":[
            {"type":"function","name":"read"},
            {"type":"custom","name":"patch"},
            {"type":"function","name":"nested.read"}
        ]},
        {"type":"namespace","name":"files.nested","tools":[
            {"type":"function","name":"write"}
        ]},
        {"type":"namespace","name":"other","tools":[
            {"type":"function","name":"read"},
            {"type":"function","name":"solo"}
        ]},
        {"type":"namespace","name":"Файлы.深い","tools":[
            {"type":"function","name":"чтение"}
        ]},
        {"type":"function","name":"standalone"},
        {"type":"function","name":"flat.dotted"}
    ]}))
    .unwrap()
}

const NAMES: &[&str] = &[
    "read",
    "files.read",
    "filesread",
    "files..read",
    "Files.read",
    "files.READ",
    "files.nested.read",
    "files.nested.write",
    "flat.dotted",
    "other.solo",
    "solo",
    "standalone",
    "patch",
    "Файлы.深い.чтение",
    "файлы.深い.чтение",
    "",
    "unknown",
    " files.read",
    "files.read ",
];

#[test]
fn exact_key_matching_preserves_materialized_bytes_without_normalization() {
    let template = identity_catalog().remove(0);
    for namespace in [
        None,
        Some(""),
        Some("files"),
        Some("files.nested"),
        Some("Файлы.深い"),
        Some("a."),
    ] {
        for name in [
            "",
            "read",
            "nested.read",
            ".read",
            "read.",
            "café",
            "cafe\u{301}",
            "ЧТЕНИЕ",
            "read name",
        ] {
            // Exercise the string identity independently of declaration validation.
            let mut tool = template.clone();
            tool.namespace = namespace.map(str::to_owned);
            tool.name = name.to_string();
            let key = tool.key();
            for candidate in [
                key.clone(),
                name.to_string(),
                format!("{key}."),
                format!(".{key}"),
                key.to_lowercase(),
                key.to_uppercase(),
                format!(" {key}"),
                format!("{key} "),
                key.replace('.', "_"),
                String::new(),
            ] {
                assert_eq!(
                    tool.matches_key(&candidate),
                    key == candidate,
                    "{key:?} vs {candidate:?}"
                );
            }
        }
    }
}

#[test]
fn resolution_preserves_exact_names_and_rejects_ambiguous_bare_names() {
    let mut tools = identity_catalog();
    for _ in 0..2 {
        for &name in NAMES {
            match (tool_spec(&tools, name), original_tool_spec(&tools, name)) {
                (Some(actual), Some(expected)) => assert!(std::ptr::eq(actual, expected)),
                (None, None) => {}
                _ => panic!("identity mismatch for {name:?}"),
            }
        }
        assert!(tool_spec(&tools, "read").is_none());
        assert_eq!(tool_spec(&tools, "solo").unwrap().key(), "other.solo");
        assert_eq!(
            tool_spec(&tools, "files.read")
                .unwrap()
                .namespace
                .as_deref(),
            Some("files")
        );
        tools.reverse();
    }
    tools.extend(client_tools(&json!({"tools":[{"type":"function","name":"read"}]})).unwrap());
    assert!(tool_spec(&tools, "read").unwrap().namespace.is_none());
}

#[test]
fn selection_preserves_kind_namespace_and_current_tool_choice() {
    let tools = identity_catalog();
    let assert_selection = |request: Value| {
        let actual = selected_tools(&request, &tools);
        let expected = original_selected_tools(&request, &tools);
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.key(), expected.key());
            assert_eq!(actual.kind, expected.kind);
            assert!(Arc::ptr_eq(&actual.spec, &expected.spec));
        }
    };
    assert_selection(json!({}));
    for choice in [
        json!("none"),
        json!("auto"),
        json!("required"),
        json!({"type":"auto"}),
    ] {
        assert_selection(json!({"tool_choice":choice}));
    }
    for &name in NAMES {
        for kind in [None, Some("function"), Some("custom"), Some("unknown")] {
            for namespace in [
                None,
                Some("files"),
                Some("files.nested"),
                Some("other"),
                Some(""),
            ] {
                let mut choice = json!({"name":name});
                if let Some(kind) = kind {
                    choice["type"] = json!(kind);
                }
                if let Some(namespace) = namespace {
                    choice["namespace"] = json!(namespace);
                }
                for tool in &tools {
                    assert_eq!(
                        choice_matches_tool(&choice, tool),
                        original_choice_matches(&choice, tool)
                    );
                }
                assert_selection(json!({"tool_choice":choice}));
                assert_selection(json!({"tool_choice":{"type":"allowed_tools","tools":[choice]}}));
            }
        }
    }
    assert_eq!(
        selected_tools(
            &json!({"tool_choice":{"type":"function","name":"read"}}),
            &tools
        )
        .len(),
        2
    );
    assert!(selected_tools(&json!({"tool_choice":"none"}), &tools).is_empty());
}

#[test]
#[ignore = "paired synthetic tool identity/selection benchmark; not provider or request latency"]
fn measure_tool_identity_paired() {
    use sha2::{Digest, Sha256};
    use std::{hint::black_box, time::Instant};

    const NAMESPACE: &str = "synthetic.operations.workspace";
    for count in [24, 256, 1024] {
        let tools = client_tools(&json!({"tools":[{
            "type":"namespace","name":NAMESPACE,"tools":(0..count).map(|index| json!({
                "type":"function","name":format!("operation_{index:04}")
            })).collect::<Vec<_>>()
        }]}))
        .unwrap();
        let request = json!({"tool_choice":{"type":"allowed_tools","tools":(count-8..count)
            .map(|index| json!({"type":"function","name":format!("{NAMESPACE}.operation_{index:04}")}))
            .collect::<Vec<_>>()}});
        for case in ["qualified", "missing", "allowed"] {
            let name = if case == "missing" {
                format!("{NAMESPACE}.missing")
            } else {
                format!("{NAMESPACE}.operation_{:04}", count - 1)
            };
            let keys = |current| {
                if case == "allowed" {
                    let selected = if current {
                        selected_tools(&request, &tools)
                    } else {
                        original_selected_tools(&request, &tools)
                    };
                    selected.iter().map(ClientTool::key).collect::<Vec<_>>()
                } else {
                    let found = if current {
                        tool_spec(&tools, &name)
                    } else {
                        original_tool_spec(&tools, &name)
                    };
                    found.into_iter().map(ClientTool::key).collect()
                }
            };
            let expected = keys(false);
            assert_eq!(keys(true), expected);
            let iterations = if case == "allowed" { 20 } else { 100 };
            let measure = |current| {
                let started = Instant::now();
                for _ in 0..iterations {
                    let tools = black_box(tools.as_slice());
                    if case == "allowed" {
                        if current {
                            black_box(selected_tools(black_box(&request), tools));
                        } else {
                            black_box(original_selected_tools(black_box(&request), tools));
                        }
                    } else if current {
                        black_box(tool_spec(tools, black_box(&name)));
                    } else {
                        black_box(original_tool_spec(tools, black_box(&name)));
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
                "tool-identity tools={count} case={case} iterations={iterations} reference_us={} current_us={} result_sha256={}",
                before[4].as_micros(),
                after[4].as_micros(),
                hex::encode(Sha256::digest(serde_json::to_vec(&expected).unwrap()))
            );
        }
    }
}
