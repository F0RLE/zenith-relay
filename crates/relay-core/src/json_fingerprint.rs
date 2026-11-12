//! Stable JSON fingerprints without allocating a complete serialized copy.
use serde::Serialize;
use serde::ser::{SerializeMap, SerializeSeq};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, BufWriter, Write};

pub(crate) fn sha256(value: &impl Serialize) -> Result<[u8; 32], serde_json::Error> {
    let mut digest = Sha256::new();
    {
        let mut writer = BufWriter::with_capacity(8 * 1024, DigestWriter(&mut digest));
        serde_json::to_writer(&mut writer, value)?;
        writer.flush().map_err(serde_json::Error::io)?;
    }
    Ok(digest.finalize().into())
}

/// Compare JSON structure without changing the outgoing object's key order.
pub(crate) fn canonical_sha256(value: &Value) -> Result<[u8; 32], serde_json::Error> {
    sha256(&Canonical(value))
}

struct Canonical<'a>(&'a Value);

impl Serialize for Canonical<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, &Canonical(value))?;
                }
                map.end()
            }
            Value::Array(items) => {
                let mut sequence = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    sequence.serialize_element(&Canonical(item))?;
                }
                sequence.end()
            }
            value => value.serialize(serializer),
        }
    }
}

struct DigestWriter<'a>(&'a mut Sha256);

impl Write for DigestWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn streaming_matches_buffered_json_for_escapes_numbers_and_large_payloads() {
        for value in [
            Value::Null,
            json!({"text":"Synthetic \"quotes\", \\paths,\r\n\tкириллица\u{0}","array":[true,1.25,null]}),
            serde_json::from_str::<Value>(r#"{"large":90071992547409931234567,"small":1e-100}"#)
                .unwrap(),
            json!({"synthetic_image":"abc/xyz+0123=".repeat(100_000)}),
        ] {
            let expected: [u8; 32] = Sha256::digest(serde_json::to_vec(&value).unwrap()).into();
            assert_eq!(sha256(&value).unwrap(), expected);
        }
        let slice = [json!("first"), json!({"second":2})];
        assert_eq!(
            sha256(&slice.as_slice()).unwrap(),
            sha256(&json!(slice)).unwrap()
        );
    }

    #[test]
    fn serialization_failure_is_returned_instead_of_a_partial_fingerprint() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("synthetic serialization failure"))
            }
        }
        assert!(sha256(&Invalid).is_err());
    }

    #[test]
    fn canonical_fingerprints_ignore_only_object_key_order() {
        let original: Value =
            serde_json::from_str(r#"{"z":[{"b":2,"a":1},"opaque {\"b\":2,\"a\":1}"],"a":true}"#)
                .unwrap();
        let original_bytes = serde_json::to_vec(&original).unwrap();
        let mut sorted = original.clone();
        sorted.sort_all_objects();
        assert_eq!(
            canonical_sha256(&original).unwrap(),
            sha256(&sorted).unwrap()
        );
        assert_eq!(
            canonical_sha256(&original).unwrap(),
            canonical_sha256(&sorted).unwrap()
        );
        assert_eq!(serde_json::to_vec(&original).unwrap(), original_bytes);
        let mut changed = original.clone();
        changed["z"].as_array_mut().unwrap().reverse();
        assert_ne!(
            canonical_sha256(&original).unwrap(),
            canonical_sha256(&changed).unwrap()
        );
        changed = original.clone();
        changed["z"][1] = json!("opaque {\"a\":1,\"b\":2}");
        assert_ne!(
            canonical_sha256(&original).unwrap(),
            canonical_sha256(&changed).unwrap()
        );
    }
}
