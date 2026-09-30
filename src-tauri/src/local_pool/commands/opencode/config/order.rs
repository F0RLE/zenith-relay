use serde::{
    ser::{SerializeMap, SerializeSeq, Serializer},
    Serialize,
};
use serde_json::{Map, Value};

/// OpenCode keeps its generated effort keys, then appends config keys in file
/// order. A normal `serde_json` object is a `BTreeMap`, so `max` is written
/// before `xhigh` and the OpenCode menu follows that order.
pub(in crate::local_pool::commands::opencode) fn serialize_config(
    config: &Map<String, Value>,
) -> Result<String, serde_json::Error> {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut buffer,
        serde_json::ser::PrettyFormatter::with_indent(b"  "),
    );
    OrderedJsonObject {
        object: config,
        parent_key: None,
    }
    .serialize(&mut serializer)?;
    String::from_utf8(buffer).map_err(|error| {
        serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

struct OrderedJson<'a> {
    value: &'a Value,
    parent_key: Option<&'a str>,
}

impl Serialize for OrderedJson<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.value {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(value) => serializer.serialize_bool(*value),
            Value::Number(value) => value.serialize(serializer),
            Value::String(value) => serializer.serialize_str(value),
            Value::Array(values) => {
                let mut array = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    array.serialize_element(&Self {
                        value,
                        parent_key: None,
                    })?;
                }
                array.end()
            }
            Value::Object(object) => OrderedJsonObject {
                object,
                parent_key: self.parent_key,
            }
            .serialize(serializer),
        }
    }
}

struct OrderedJsonObject<'a> {
    object: &'a Map<String, Value>,
    parent_key: Option<&'a str>,
}

impl Serialize for OrderedJsonObject<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let keys = if self.parent_key == Some("variants") {
            ordered_variant_keys(self.object)
        } else {
            self.object.keys().collect()
        };
        let mut map = serializer.serialize_map(Some(keys.len()))?;
        for key in keys {
            map.serialize_entry(
                key,
                &OrderedJson {
                    value: &self.object[key],
                    parent_key: Some(key.as_str()),
                },
            )?;
        }
        map.end()
    }
}

fn ordered_variant_keys(object: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort_by(|left, right| {
        zenith_relay_core::reasoning_level_rank(left)
            .cmp(&zenith_relay_core::reasoning_level_rank(right))
            .then_with(|| left.as_str().cmp(right.as_str()))
    });
    keys
}
