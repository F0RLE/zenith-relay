use serde_json::{Map, Value};

const MAX_EXPANSION_BYTES: usize = 1024 * 1024;
const MAX_EXPANSION_DEPTH: usize = 64;

/// Inline ordinary local definitions for account tool compatibility. Expansion
/// is optional: if it would change reference scope or exceed the memory budget,
/// keep the original schema, including all definitions and constraints.
pub(super) fn inline_local_refs(value: &mut Value) {
    let mut budget = MAX_EXPANSION_BYTES;
    if !charge(value, &mut budget) {
        return;
    }
    if !value.get("$defs").is_some_and(Value::is_object)
        && !value.get("definitions").is_some_and(Value::is_object)
    {
        return;
    }
    let mut expanded = value.clone();
    if expand(&mut expanded, value, &mut Vec::new(), &mut budget, 0).is_none() {
        return;
    }
    if !contains_ref(&expanded) {
        if let Some(object) = expanded.as_object_mut() {
            object.remove("$defs");
            object.remove("definitions");
        }
    }
    *value = expanded;
}

// Account for strings, keys and tree nodes before cloning a referenced subtree.
// A small DAG can otherwise expand exponentially despite the HTTP body limit.
fn charge(value: &Value, remaining: &mut usize) -> bool {
    let cost = std::mem::size_of::<Value>() + value.as_str().map_or(0, str::len);
    let Some(left) = remaining.checked_sub(cost) else {
        return false;
    };
    *remaining = left;
    match value {
        Value::Object(object) => object.iter().all(|(key, child)| {
            let Some(left) = remaining.checked_sub(key.len()) else {
                return false;
            };
            *remaining = left;
            charge(child, remaining)
        }),
        Value::Array(values) => values.iter().all(|child| charge(child, remaining)),
        _ => true,
    }
}

fn expand(
    value: &mut Value,
    root: &Value,
    stack: &mut Vec<String>,
    budget: &mut usize,
    depth: usize,
) -> Option<()> {
    if depth > MAX_EXPANSION_DEPTH {
        return None;
    }
    let Some(object) = value.as_object_mut() else {
        return Some(());
    };
    // Resource identifiers and dynamic references need a full dialect-aware
    // resolver; preserving them is safer than rewriting their lexical scope.
    if [
        "id",
        "$id",
        "$anchor",
        "$dynamicAnchor",
        "$dynamicRef",
        "$recursiveAnchor",
        "$recursiveRef",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
    {
        return None;
    }
    let reference = object
        .get("$ref")
        .and_then(Value::as_str)
        .filter(|reference| {
            reference.starts_with("#/$defs/") || reference.starts_with("#/definitions/")
        })
        .filter(|_| {
            object
                .keys()
                .all(|key| matches!(key.as_str(), "$ref" | "title" | "description"))
        })
        .map(str::to_owned);
    if let Some(reference) = reference {
        if !stack.contains(&reference) {
            if let Some(target) = root.pointer(&reference[1..]) {
                if !charge(target, budget) {
                    return None;
                }
                // Boolean schemas with annotations retain their reference.
                if target.is_object() || (target.is_boolean() && object.len() == 1) {
                    stack.push(reference);
                    let mut resolved = target.clone();
                    expand(&mut resolved, root, stack, budget, depth + 1)?;
                    stack.pop();
                    if let Some(resolved) = resolved.as_object_mut() {
                        for (key, sibling) in std::mem::take(object) {
                            if key != "$ref" {
                                resolved.insert(key, sibling);
                            }
                        }
                    }
                    *value = resolved;
                    return Some(());
                }
            }
        }
    }
    visit_subschemas(object, |child| {
        expand(child, root, stack, budget, depth + 1)
    })
}

fn visit_subschemas(
    object: &mut Map<String, Value>,
    mut visit: impl FnMut(&mut Value) -> Option<()>,
) -> Option<()> {
    // These are schema positions. Defaults, examples, enum/const values and
    // extension data are ordinary JSON and must never be interpreted as schemas.
    for key in [
        "$defs",
        "definitions",
        "properties",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(map) = object.get_mut(key).and_then(Value::as_object_mut) {
            for child in map.values_mut() {
                visit(child)?;
            }
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(array) = object.get_mut(key).and_then(Value::as_array_mut) {
            for child in array {
                visit(child)?;
            }
        }
    }
    for key in [
        "items",
        "additionalItems",
        "additionalProperties",
        "unevaluatedItems",
        "unevaluatedProperties",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(child) = object.get_mut(key) {
            if let Some(items) = child.as_array_mut() {
                for item in items {
                    visit(item)?;
                }
            } else {
                visit(child)?;
            }
        }
    }
    Some(())
}

fn contains_ref(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.contains_key("$ref") || object.values().any(contains_ref),
        Value::Array(items) => items.iter().any(contains_ref),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_examples_and_constraint_siblings_are_preserved() {
        let mut schema = json!({
            "$defs":{"number":{"type":"integer","minimum":10}},
            "properties":{
                "plain":{"$ref":"#/$defs/number"},
                "constrained":{"$ref":"#/$defs/number","minimum":0},
                "data":{"default":{"$ref":"#/$defs/number"},
                    "examples":[{"$ref":"#/$defs/number"}]}
            }
        });
        let original = schema.clone();
        inline_local_refs(&mut schema);
        assert_eq!(schema["properties"]["plain"], original["$defs"]["number"]);
        for key in ["constrained", "data"] {
            assert_eq!(schema["properties"][key], original["properties"][key]);
        }
        assert_eq!(schema["$defs"], original["$defs"]);
    }

    #[test]
    fn branching_and_deep_references_fall_back_atomically() {
        for (width, depth) in [(1, 80), (2, 20)] {
            let mut definitions = Map::from_iter([("leaf".into(), json!({"type":"string"}))]);
            let mut previous = "leaf".to_owned();
            for index in 0..depth {
                let id = format!("node-{index}");
                let children = (0..width)
                    .map(|_| json!({"$ref":format!("#/$defs/{previous}")}))
                    .collect::<Vec<_>>();
                definitions.insert(id.clone(), json!({"allOf":children}));
                previous = id;
            }
            let mut schema = json!({"$defs":definitions,"properties":{"value":{"$ref":format!("#/$defs/{previous}")}}});
            let original = schema.clone();
            inline_local_refs(&mut schema);
            assert_eq!(schema, original);
        }
    }

    #[test]
    fn nested_resource_scope_and_dynamic_references_are_not_rewritten() {
        for keyword in ["id", "$id", "$anchor", "$dynamicRef", "$recursiveAnchor"] {
            let mut schema = json!({"$defs":{"value":{"type":"string"}},"properties":{"value":{"$ref":"#/$defs/value"}}});
            schema["$defs"]["value"][keyword] = json!("synthetic");
            let original = schema.clone();
            inline_local_refs(&mut schema);
            assert_eq!(schema, original);
        }
    }
}
