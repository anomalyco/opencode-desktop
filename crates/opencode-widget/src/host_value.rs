//! The VM's value, as a host function receives it ([`HostValue`]), read straight into this
//! crate's types (a view's [`Node`](crate::Node), draw commands) exactly as its JSON would be
//! read (`HostValue::to_json`), without making that JSON: `undefined` and functions are left out
//! of objects and are `null` in arrays, a whole number under 9e15 is an integer, one that isn't
//! finite is `null`. Anything else (a Map, a Date, a Set, bytes) is read from its own JSON, so
//! every value gives the same result and the same errors as before.

use std::borrow::Cow;

use psychoscript::HostValue;
use serde::de::{self, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::draw::Arg;

const NULL: &HostValue = &HostValue::Null;

/// What JSON leaves out of an object and writes as `null` in an array.
fn skipped(v: &HostValue) -> bool {
    matches!(v, HostValue::Undefined | HostValue::Function(_) | HostValue::FunctionMarker | HostValue::Durable(_) | HostValue::Namespace(_) | HostValue::Interrupt)
}

/// A number as JSON has it: an integer, a float, or (not finite) `null`.
enum Num {
    Int(i64),
    Float(f64),
    Null,
}

fn num(n: f64) -> Num {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        Num::Int(n as i64)
    } else if n.is_finite() {
        Num::Float(n)
    } else {
        Num::Null
    }
}

/// Whether `v` holds only what's read here directly (else it's read from its JSON).
pub(crate) fn plain(v: &HostValue) -> bool {
    match v {
        HostValue::Null | HostValue::Bool(_) | HostValue::Number(_) | HostValue::String(_) => true,
        HostValue::Array(items) => items.iter().flatten().all(plain),
        HostValue::Object(fields) => fields.iter().all(|(_, v)| plain(v)),
        v => skipped(v),
    }
}

/// Reads `T` from `v` as [`serde_json::from_value`] would from `v.to_json()`.
pub(crate) fn read<T: de::DeserializeOwned>(v: &HostValue) -> Result<T, serde_json::Error> {
    T::deserialize(De(v))
}

/// A [`Deserializer`] over a `HostValue`, as `serde_json::Value`'s over its JSON.
#[derive(Clone, Copy)]
struct De<'a>(&'a HostValue);

impl De<'_> {
    /// Its JSON, for what isn't read directly.
    fn json(self) -> Value {
        self.0.to_json().unwrap_or(Value::Null)
    }
}

impl De<'_> {
    /// Whether it's read here; anything else is read from its JSON.
    fn direct(self) -> bool {
        matches!(self.0, HostValue::Null | HostValue::Bool(_) | HostValue::Number(_) | HostValue::String(_) | HostValue::Array(_) | HostValue::Object(_)) || skipped(self.0)
    }

    /// Whether its JSON is `null`.
    fn null(self) -> bool {
        skipped(self.0) || matches!(self.0, HostValue::Null) || matches!(self.0, HostValue::Number(n) if !n.is_finite())
    }

    /// The error a deserializer asking for something else gives, as `serde_json::Value`'s does.
    fn invalid(self, exp: &dyn de::Expected) -> serde_json::Error {
        let s;
        let what = match self.0 {
            _ if self.null() => de::Unexpected::Unit,
            HostValue::Bool(b) => de::Unexpected::Bool(*b),
            HostValue::Number(n) => match num(*n) {
                Num::Int(i) if i >= 0 => de::Unexpected::Unsigned(i as u64),
                Num::Int(i) => de::Unexpected::Signed(i),
                _ => de::Unexpected::Float(*n),
            },
            HostValue::String(x) => {
                s = x.to_string_lossy();
                de::Unexpected::Str(&s)
            }
            HostValue::Array(_) => de::Unexpected::Seq,
            _ => de::Unexpected::Map,
        };
        de::Error::invalid_type(what, exp)
    }

    fn seq<'de, V: Visitor<'de>>(items: &[Option<HostValue>], visitor: V) -> Result<V::Value, serde_json::Error> {
        let mut seq = Seq { items: items.iter(), left: items.len() };
        let out = visitor.visit_seq(&mut seq)?;
        match seq.left {
            0 => Ok(out),
            _ => Err(de::Error::invalid_length(items.len(), &"fewer elements in array")),
        }
    }

    fn map<'de, V: Visitor<'de>>(fields: &[(psychoscript::HostString, HostValue)], visitor: V) -> Result<V::Value, serde_json::Error> {
        let mut map = Fields { fields: fields.iter(), value: None };
        let out = visitor.visit_map(&mut map)?;
        match map.fields.any(|(_, v)| !skipped(v)) {
            false => Ok(out),
            true => Err(de::Error::invalid_length(fields.len(), &"fewer elements in map")),
        }
    }

    fn string<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, serde_json::Error> {
        match self.0 {
            HostValue::String(s) => match s.to_string_lossy() {
                Cow::Borrowed(s) => visitor.visit_str(s),
                Cow::Owned(s) => visitor.visit_string(s),
            },
            _ => Err(self.invalid(&visitor)),
        }
    }

    fn number<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, serde_json::Error> {
        match self.0 {
            HostValue::Number(n) => match num(*n) {
                Num::Int(i) if i >= 0 => visitor.visit_u64(i as u64),
                Num::Int(i) => visitor.visit_i64(i),
                Num::Float(f) => visitor.visit_f64(f),
                Num::Null => Err(self.invalid(&visitor)),
            },
            _ => Err(self.invalid(&visitor)),
        }
    }
}

/// Mirrors `serde_json::Value`'s deserializer, method by method, on the value's JSON.
macro_rules! json_else {
    ($self:ident, $method:ident($($arg:expr),*), $visitor:ident) => {
        if !$self.direct() {
            return $self.json().$method($($arg,)* $visitor);
        }
    };
}

macro_rules! numbers {
    ($($method:ident)*) => {
        $(fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            json_else!(self, $method(), visitor);
            self.number(visitor)
        })*
    };
}

impl<'de> Deserializer<'de> for De<'_> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_any(), visitor);
        match self.0 {
            _ if self.null() => visitor.visit_unit(),
            HostValue::Bool(b) => visitor.visit_bool(*b),
            HostValue::Number(_) => self.number(visitor),
            HostValue::String(_) => self.string(visitor),
            HostValue::Array(items) => De::seq(items, visitor),
            HostValue::Object(fields) => De::map(fields, visitor),
            _ => unreachable!("read from its JSON above"),
        }
    }

    numbers! { deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128 deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128 deserialize_f32 deserialize_f64 }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_bool(), visitor);
        match self.0 {
            HostValue::Bool(b) => visitor.visit_bool(*b),
            _ => Err(self.invalid(&visitor)),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_char(), visitor);
        self.string(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_str(), visitor);
        self.string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_string(), visitor);
        self.string(visitor)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_identifier(), visitor);
        self.string(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_bytes(), visitor);
        match self.0 {
            HostValue::String(_) => self.string(visitor),
            HostValue::Array(items) => De::seq(items, visitor),
            _ => Err(self.invalid(&visitor)),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_option(), visitor);
        if self.null() { visitor.visit_none() } else { visitor.visit_some(self) }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_unit(), visitor);
        if self.null() { visitor.visit_unit() } else { Err(self.invalid(&visitor)) }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_newtype_struct(name), visitor);
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_seq(), visitor);
        match self.0 {
            HostValue::Array(items) => De::seq(items, visitor),
            _ => Err(self.invalid(&visitor)),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _name: &'static str, _len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_map(), visitor);
        match self.0 {
            HostValue::Object(fields) => De::map(fields, visitor),
            _ => Err(self.invalid(&visitor)),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(self, name: &'static str, fields: &'static [&'static str], visitor: V) -> Result<V::Value, Self::Error> {
        json_else!(self, deserialize_struct(name, fields), visitor);
        match self.0 {
            HostValue::Array(items) => De::seq(items, visitor),
            HostValue::Object(fields) => De::map(fields, visitor),
            _ => Err(self.invalid(&visitor)),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(self, name: &'static str, variants: &'static [&'static str], visitor: V) -> Result<V::Value, Self::Error> {
        self.json().deserialize_enum(name, variants, visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }
}

struct Seq<'a> {
    items: std::slice::Iter<'a, Option<HostValue>>,
    left: usize,
}

impl<'de> SeqAccess<'de> for Seq<'_> {
    type Error = serde_json::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        match self.items.next() {
            Some(item) => {
                self.left -= 1;
                seed.deserialize(De(item.as_ref().unwrap_or(NULL))).map(Some)
            }
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

struct Fields<'a> {
    fields: std::slice::Iter<'a, (psychoscript::HostString, HostValue)>,
    value: Option<&'a HostValue>,
}

impl<'de> MapAccess<'de> for Fields<'_> {
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        for (k, v) in self.fields.by_ref() {
            if skipped(v) {
                continue;
            }
            self.value = Some(v);
            return match k.to_string_lossy() {
                Cow::Borrowed(k) => seed.deserialize(IntoDeserializer::<serde_json::Error>::into_deserializer(k)).map(Some),
                Cow::Owned(k) => seed.deserialize(IntoDeserializer::<serde_json::Error>::into_deserializer(k)).map(Some),
            };
        }
        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        seed.deserialize(De(self.value.take().unwrap_or(NULL)))
    }
}

/// Draw commands read from a plain `HostValue` (see [`plain`]) as from its JSON.
impl Arg for HostValue {
    fn number(&self) -> Option<f64> {
        match self {
            HostValue::Number(n) => match num(*n) {
                Num::Int(i) => Some(i as f64),
                Num::Float(f) => Some(f),
                Num::Null => None,
            },
            _ => None,
        }
    }
    fn text(&self) -> Option<Cow<'_, str>> {
        match self {
            HostValue::String(s) => Some(s.to_string_lossy()),
            _ => None,
        }
    }
    fn flag(&self) -> Option<bool> {
        match self {
            HostValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn is_null(&self) -> bool {
        matches!(self, HostValue::Null) || skipped(self) || matches!(self, HostValue::Number(n) if !n.is_finite())
    }
    fn len(&self) -> Option<usize> {
        match self {
            HostValue::Array(items) => Some(items.len()),
            _ => None,
        }
    }
    fn at(&self, i: usize) -> Option<&Self> {
        match self {
            HostValue::Array(items) => items.get(i).map(|item| item.as_ref().unwrap_or(NULL)),
            _ => None,
        }
    }
    fn field(&self, key: &str) -> Option<&Self> {
        match self {
            HostValue::Object(fields) => fields.iter().find(|(k, v)| !skipped(v) && k.to_string_lossy() == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn is_object(&self) -> bool {
        matches!(self, HostValue::Object(_))
    }
}
