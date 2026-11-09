//! Luau has one table type, so an empty list a plugin passes (`{}`) reaches a host as an empty
//! map. The generated records' list fields take one as an empty list.

use serde::de::{Deserialize, DeserializeOwned, Deserializer, Error};
use serde_json::Value;

/// A list field: an array, or an empty table.
pub fn seq<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    list(Value::deserialize(d)?).map_err(D::Error::custom)
}

/// An optional list field: absent or null, an array, or an empty table.
pub fn opt_seq<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Option<Vec<T>>, D::Error> {
    match Value::deserialize(d)? {
        Value::Null => Ok(None),
        v => list(v).map(Some).map_err(D::Error::custom),
    }
}

/// `value` as a list: an empty object reads as an empty array.
pub fn list<T: DeserializeOwned>(value: Value) -> Result<Vec<T>, serde_json::Error> {
    match value {
        Value::Object(m) if m.is_empty() => Ok(Vec::new()),
        v => serde_json::from_value(v),
    }
}
