use super::*;
use serde_json::json;

fn declaration(name: &str, minimum: i64) -> Value {
    json!({"type":"function","name":name,"parameters":{
        "type":"object","properties":{"value":{"type":"integer","minimum":minimum}},
        "required":["value"],"additionalProperties":false
    }})
}

#[test]
fn resolves_overrides_before_preparing_immutable_declarations() {
    let request = json!({
        "input":[
            {"type":"additional_tools","tools":[
                declaration("first", 1),
                {"type":"namespace","name":"nested","tools":[declaration("tool", 2)]}
            ]},
            {"type":"tool_search_output","tools":[declaration("first", 3), declaration("last", 4)]}
        ],
        "tools":[{"type":"namespace","name":"nested","tools":[declaration("tool", 5)]}]
    });
    let mut declarations = Vec::new();
    let mut positions = HashMap::new();
    for item in request["input"].as_array().unwrap() {
        collect_tools(item.get("tools"), None, &mut declarations, &mut positions).unwrap();
    }
    collect_tools(
        request.get("tools"),
        None,
        &mut declarations,
        &mut positions,
    )
    .unwrap();
    assert_eq!(declarations.len(), 3);
    assert!(std::ptr::eq(
        declarations[0].spec,
        request["input"][1]["tools"][0].as_object().unwrap()
    ));
    assert!(std::ptr::eq(
        declarations[1].spec,
        request["tools"][0]["tools"][0].as_object().unwrap()
    ));
    let tools = client_tools(&request).unwrap();
    assert_eq!(
        tools.iter().map(ClientTool::key).collect::<Vec<_>>(),
        ["first", "nested.tool", "last"]
    );
    assert_eq!(
        tools[0].spec["parameters"]["properties"]["value"]["minimum"],
        3
    );
    assert_eq!(
        tools[1].spec["parameters"]["properties"]["value"]["minimum"],
        5
    );
    assert_eq!(
        declared_tools(request.get("tools")).unwrap()[0].key(),
        "nested.tool"
    );
}

#[test]
fn overridden_malformed_declarations_are_still_rejected() {
    for invalid in [
        json!({"type":"function"}),
        json!({"type":"unsupported","name":"first"}),
        json!({"type":"namespace","name":"nested"}),
    ] {
        let request = json!({"input":[{"type":"additional_tools","tools":[invalid]}],"tools":[declaration("first", 1)]});
        assert!(client_tools(&request).is_err());
    }
}

#[test]
fn successful_schema_preparation_is_shared_but_never_grants_permission() {
    let request = json!({"tools":[declaration("synthetic_cached_schema_success", 1)]});
    let first = client_tools(&request).unwrap().remove(0);
    first.validate_parameter_schema().unwrap();
    let second = client_tools(&request).unwrap().remove(0);
    assert!(Arc::ptr_eq(
        &first.schema_validated,
        &second.schema_validated
    ));
    assert_eq!(second.schema_validated.get(), Some(&()));
    second.validate_parameter_schema().unwrap();
    assert!(
        selected_tools(
            &json!({"tool_choice":"none"}),
            std::slice::from_ref(&second)
        )
        .is_empty()
    );
    let schema = super::super::schema::parameter_schema(&second).unwrap();
    assert!(super::super::schema::validate(&json!({"value":1}), schema).is_ok());
    assert!(super::super::schema::validate(&json!({"value":0}), schema).is_err());
    let mut changed = request;
    changed["tools"][0]["parameters"]["properties"]["value"]["type"] =
        json!("synthetic-invalid-type");
    let changed = client_tools(&changed).unwrap().remove(0);
    assert!(!Arc::ptr_eq(
        &first.schema_validated,
        &changed.schema_validated
    ));
    for _ in 0..2 {
        assert!(changed.validate_parameter_schema().is_err());
        assert!(changed.schema_validated.get().is_none());
    }
}

// The former strategy prepared each historical version before replacing it.
fn eager_catalog(request: &Value) -> Vec<ClientTool> {
    let mut result = Vec::new();
    let mut positions = HashMap::<String, usize>::new();
    let declarations = request["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.get("tools"))
        .chain(std::iter::once(request.get("tools")));
    for tools in declarations {
        for tool in declared_tools(tools).unwrap() {
            let key = tool.key();
            if let Some(&position) = positions.get(&key) {
                result[position] = tool;
            } else {
                positions.insert(key, result.len());
                result.push(tool);
            }
        }
    }
    result
}

#[test]
#[ignore = "manual synthetic historical declaration and warm schema preparation measurement"]
fn measure_catalog_history_preparation() {
    use std::{hint::black_box, time::Instant};

    let tools = (0..32)
        .map(|index| {
            let mut tool = declaration(&format!("synthetic_catalog_benchmark_{index}"), 1);
            tool["description"] = json!("Synthetic tool description. ".repeat(32));
            tool
        })
        .collect::<Vec<_>>();
    let request = json!({"input":(0..12).map(|_| json!({"type":"additional_tools","tools":tools})).collect::<Vec<_>>(),"tools":tools});
    let expected = eager_catalog(&request);
    let actual = client_tools(&request).unwrap();
    assert_eq!(
        actual.iter().map(ClientTool::key).collect::<Vec<_>>(),
        expected.iter().map(ClientTool::key).collect::<Vec<_>>()
    );
    for (actual, expected) in actual.iter().zip(&expected) {
        assert_eq!(actual.spec, expected.spec);
        actual.validate_parameter_schema().unwrap();
    }
    let mut eager = Vec::new();
    let mut resolved = Vec::new();
    let mut repeated_validation = Vec::new();
    let mut shared_validation = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..10 {
            black_box(eager_catalog(black_box(&request)));
        }
        eager.push(start.elapsed());
        let start = Instant::now();
        for _ in 0..10 {
            black_box(client_tools(black_box(&request)).unwrap());
        }
        resolved.push(start.elapsed());
        let start = Instant::now();
        for _ in 0..100 {
            for tool in &actual {
                black_box(
                    super::super::schema::validator(
                        super::super::schema::parameter_schema(tool).unwrap(),
                    )
                    .unwrap(),
                );
            }
        }
        repeated_validation.push(start.elapsed());
        let start = Instant::now();
        for _ in 0..100 {
            for tool in &actual {
                black_box(tool).validate_parameter_schema().unwrap();
            }
        }
        shared_validation.push(start.elapsed());
    }
    for samples in [
        &mut eager,
        &mut resolved,
        &mut repeated_validation,
        &mut shared_validation,
    ] {
        samples.sort_unstable();
    }
    eprintln!(
        "catalog fixture_bytes={}, declarations=416, prepared=32, median/10: eager={:?}, resolved={:?}; warm schema median/3200: cache_lookup={:?}, shared_success={:?}",
        request.to_string().len(),
        eager[2],
        resolved[2],
        repeated_validation[2],
        shared_validation[2]
    );
}
