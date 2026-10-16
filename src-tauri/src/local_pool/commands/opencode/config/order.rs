use serde::{
    ser::{SerializeMap, SerializeSeq, Serializer},
    Serialize,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// OpenCode keeps its generated effort keys, then appends config keys in file
/// order. A normal `serde_json` object is a `BTreeMap`, so `max` is written
/// before `xhigh` and the OpenCode menu follows that order.
#[cfg(test)]
pub(in crate::local_pool::commands::opencode) fn serialize_config(
    config: &Map<String, Value>,
) -> Result<String, serde_json::Error> {
    serialize_config_with_model_order(config, &[])
}

pub(in crate::local_pool::commands::opencode) fn serialize_config_with_model_order(
    config: &Map<String, Value>,
    models: &[String],
) -> Result<String, serde_json::Error> {
    let model_positions = models
        .iter()
        .enumerate()
        .map(|(position, model)| (zenith_relay_core::model_id_key(model), position))
        .collect();
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut buffer,
        serde_json::ser::PrettyFormatter::with_indent(b"  "),
    );
    OrderedJsonObject {
        object_fields: config,
        parent_key: None,
        managed_provider: false,
        model_positions: &model_positions,
    }
    .serialize(&mut serializer)?;
    String::from_utf8(buffer).map_err(|error| {
        serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

struct OrderedJson<'a> {
    json_value: &'a Value,
    parent_key: Option<&'a str>,
    managed_provider: bool,
    model_positions: &'a BTreeMap<String, usize>,
}

impl Serialize for OrderedJson<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.json_value {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(boolean_value) => serializer.serialize_bool(*boolean_value),
            Value::Number(number_value) => number_value.serialize(serializer),
            Value::String(string_value) => serializer.serialize_str(string_value),
            Value::Array(array_items) => {
                let mut array = serializer.serialize_seq(Some(array_items.len()))?;
                for array_value in array_items {
                    array.serialize_element(&Self {
                        json_value: array_value,
                        parent_key: None,
                        managed_provider: self.managed_provider,
                        model_positions: self.model_positions,
                    })?;
                }
                array.end()
            }
            Value::Object(object_fields) => OrderedJsonObject {
                object_fields,
                parent_key: self.parent_key,
                managed_provider: self.managed_provider,
                model_positions: self.model_positions,
            }
            .serialize(serializer),
        }
    }
}

struct OrderedJsonObject<'a> {
    object_fields: &'a Map<String, Value>,
    parent_key: Option<&'a str>,
    managed_provider: bool,
    model_positions: &'a BTreeMap<String, usize>,
}

impl Serialize for OrderedJsonObject<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut keys: Vec<_> = if self.parent_key == Some("variants") {
            ordered_variant_keys(self.object_fields)
        } else {
            self.object_fields.keys().collect()
        };
        if self.parent_key == Some("models") && self.managed_provider {
            keys.sort_by_key(|key| {
                self.model_positions
                    .get(&zenith_relay_core::model_id_key(key))
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        let mut map = serializer.serialize_map(Some(keys.len()))?;
        for key in keys {
            map.serialize_entry(
                key,
                &OrderedJson {
                    json_value: &self.object_fields[key],
                    parent_key: Some(key.as_str()),
                    managed_provider: if self.parent_key == Some("provider") {
                        super::super::protocols::managed_id(key)
                    } else {
                        self.managed_provider
                    },
                    model_positions: self.model_positions,
                },
            )?;
        }
        map.end()
    }
}

fn ordered_variant_keys(variant_object: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = variant_object.keys().collect();
    keys.sort_by(|left, right| {
        zenith_relay_core::reasoning_level_rank(left)
            .cmp(&zenith_relay_core::reasoning_level_rank(right))
            .then_with(|| left.as_str().cmp(right.as_str()))
    });
    keys
}
