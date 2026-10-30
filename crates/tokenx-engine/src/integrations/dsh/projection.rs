//! Retain accounting fields while validating every JSON value in the record.
//!
//! Transcript bodies and tool output can dwarf usage metadata. Visiting them
//! without building a `Value` tree avoids those allocations. Unlike
//! `IgnoredAny`, `deserialize_any` still validates strings and numeric ranges
//! exactly as the original `Value` decoder does.

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt;

#[derive(Clone, Copy)]
enum Projection {
    Keep,
    Discard,
    Object(&'static [(&'static str, Projection)]),
    Array(&'static Projection),
}

use Projection::{Array, Keep, Object};

const RECORD: Projection = Object(&[
    ("type", Keep),
    ("id", Keep),
    ("cwd", Keep),
    ("version", Keep),
    ("isSeeded", Keep),
    ("seedLength", Keep),
    ("seq", Keep),
    ("time", Keep),
    (
        "data",
        Object(&[
            ("inherited", Keep),
            ("turn", Keep),
            ("step", Keep),
            ("usage", Keep),
            (
                "header",
                Object(&[("config", Object(&[("provider", Keep), ("model", Keep)]))]),
            ),
            (
                "message",
                Object(&[
                    ("id", Keep),
                    (
                        "source",
                        Object(&[
                            ("provider", Keep),
                            ("model", Keep),
                            (
                                "replayState",
                                Object(&[("response", Object(&[("responseModel", Keep)]))]),
                            ),
                        ]),
                    ),
                ]),
            ),
            (
                "stream",
                Array(&Object(&[
                    ("type", Keep),
                    ("chunk", Object(&[("type", Keep), ("usage", Keep)])),
                ])),
            ),
        ]),
    ),
]);

pub(super) fn parse(line: &str) -> serde_json::Result<Value> {
    let mut decoder = serde_json::Deserializer::from_str(line);
    let value = RECORD.deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

impl<'de> DeserializeSeed<'de> for Projection {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Value, D::Error> {
        match self {
            Self::Keep => Value::deserialize(decoder),
            _ => decoder.deserialize_any(self),
        }
    }
}

impl<'de> Visitor<'de> for Projection {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        let fields = match self {
            Self::Object(fields) => fields,
            _ => &[],
        };
        let mut output = Map::new();
        while let Some(field) = input.next_key_seed(Field(fields))? {
            match field {
                Some((name, projection)) => {
                    output.insert(name.to_owned(), input.next_value_seed(projection)?);
                }
                None => {
                    input.next_value_seed(Self::Discard)?;
                }
            }
        }
        Ok(match self {
            Self::Object(_) => Value::Object(output),
            _ => Value::Null,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut input: A) -> Result<Value, A::Error> {
        match self {
            Self::Array(element) => {
                let mut output = Vec::new();
                while let Some(value) = input.next_element_seed(*element)? {
                    output.push(value);
                }
                Ok(Value::Array(output))
            }
            _ => {
                while input.next_element_seed(Self::Discard)?.is_some() {}
                Ok(Value::Null)
            }
        }
    }

    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
}

struct Field(&'static [(&'static str, Projection)]);

impl<'de> DeserializeSeed<'de> for Field {
    type Value = Option<(&'static str, Projection)>;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_str(self)
    }
}

impl Visitor<'_> for Field {
    type Value = Option<(&'static str, Projection)>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object key")
    }

    fn visit_str<E: serde::de::Error>(self, key: &str) -> Result<Self::Value, E> {
        Ok(self.0.iter().find(|(name, _)| *name == key).copied())
    }
}
