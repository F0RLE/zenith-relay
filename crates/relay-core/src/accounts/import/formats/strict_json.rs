use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::{cell::Cell, collections::HashSet, fmt};

#[derive(Debug)]
pub(super) enum ParseError {
    DuplicateField,
    Invalid,
}

pub(super) fn parse(input: &str) -> Result<Value, ParseError> {
    // Validate before materializing Value: preserve serde_json's arbitrary-precision
    // number handling without depending on its private deserializer map protocol.
    let duplicate = Cell::new(false);
    let mut deserializer = serde_json::Deserializer::from_str(input);
    UniqueFields(&duplicate)
        .deserialize(&mut deserializer)
        .map_err(|_| ParseError::Invalid)?;
    deserializer.end().map_err(|_| ParseError::Invalid)?;
    if duplicate.get() {
        return Err(ParseError::DuplicateField);
    }
    serde_json::from_str(input).map_err(|_| ParseError::Invalid)
}

struct UniqueFields<'a>(&'a Cell<bool>);

impl<'de> DeserializeSeed<'de> for UniqueFields<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(UniqueFieldsVisitor(self.0))
    }
}

struct UniqueFieldsVisitor<'a>(&'a Cell<bool>);

impl<'de> Visitor<'de> for UniqueFieldsVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object fields")
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _value: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, _value: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        while sequence.next_element_seed(UniqueFields(self.0))?.is_some() {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<(), A::Error> {
        let mut keys = HashSet::new();
        while let Some(key) = object.next_key::<String>()? {
            // MapAccess already decodes escapes, including Unicode equivalents.
            if !keys.insert(key) {
                self.0.set(true);
            }
            object.next_value_seed(UniqueFields(self.0))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_validation_preserves_arbitrary_precision_numbers() {
        let input = r#"{"fraction":1.2345678901234567890123456789,"integer":123456789012345678901234567890,"exponent":1e400}"#;
        assert_eq!(
            parse(input).unwrap(),
            serde_json::from_str::<Value>(input).unwrap()
        );
        for input in ["NaN", "Infinity", "-Infinity"] {
            assert!(matches!(parse(input), Err(ParseError::Invalid)));
        }
    }
}
