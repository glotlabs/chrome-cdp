//! Mock drift (plan 11, milestone 11.11): how a real answer and a mock's
//! answer to the same request differ in *shape*, for `test --verify-mocks`,
//! and data files rewritten from real answers for `--update`.
//!
//! [`shape`] compares keys, types (null, boolean, number, string, array,
//! object), array element shape (the first element of each side) and
//! null against present; never values. [`update`] rewrites a data file's
//! JSON from a real answer: what the real answer has and the file lacks
//! comes from the real answer, what the file has that the real answer
//! lacks goes, and a value whose type still fits is kept. Key order and
//! the file's number formatting stay as written, so the change reads as
//! a normal diff.

use std::fmt::Write as _;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

/// A JSON type's name, as the drift report says it.
pub fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// How `mock` (a mock's answer) differs in shape from `real` (the real
/// answer to the same request), one line per difference, e.g.
/// `$.items[].price_cents: the real answer has it (number); the mock has
/// "price" (number)`.
pub fn shape(real: &Value, mock: &Value) -> Vec<String> {
    let mut out = Vec::new();
    compare(real, mock, "$", &mut out);
    out
}

fn compare(real: &Value, mock: &Value, at: &str, out: &mut Vec<String>) {
    match (real, mock) {
        (Value::Object(r), Value::Object(m)) => {
            let only_real: Vec<(&String, &Value)> =
                r.iter().filter(|(k, _)| !m.contains_key(*k)).collect();
            let only_mock: Vec<(&String, &Value)> =
                m.iter().filter(|(k, _)| !r.contains_key(*k)).collect();
            // A renamed field: one on each side, of the same type.
            if let ([(rk, rv)], [(mk, mv)]) = (only_real.as_slice(), only_mock.as_slice())
                && kind(rv) == kind(mv)
            {
                out.push(format!(
                    "{at}: the real answer has field \"{rk}\" ({}); the mock has \"{mk}\" ({})",
                    kind(rv),
                    kind(mv)
                ));
            } else {
                for (k, v) in &only_real {
                    out.push(format!(
                        "{at}: the real answer has field \"{k}\" ({}); the mock has none",
                        kind(v)
                    ));
                }
                for (k, v) in &only_mock {
                    out.push(format!(
                        "{at}: the mock has field \"{k}\" ({}); the real answer has none",
                        kind(v)
                    ));
                }
            }
            for (k, rv) in r {
                if let Some(mv) = m.get(k) {
                    compare(rv, mv, &child(at, k), out);
                }
            }
        }
        (Value::Array(r), Value::Array(m)) => {
            if let (Some(rv), Some(mv)) = (r.first(), m.first()) {
                compare(rv, mv, &format!("{at}[]"), out);
            }
        }
        (Value::Null, Value::Null) => {}
        (Value::Null, m) => out.push(format!(
            "{at}: the real answer has null; the mock has a {}",
            kind(m)
        )),
        (r, Value::Null) => out.push(format!(
            "{at}: the real answer has a {}; the mock has null",
            kind(r)
        )),
        (r, m) if kind(r) != kind(m) => out.push(format!(
            "{at}: the real answer has a {}; the mock has a {}",
            kind(r),
            kind(m)
        )),
        _ => {}
    }
}

/// `$.a` then `.b`; keys that are not identifiers are quoted.
fn child(at: &str, key: &str) -> String {
    let plain = key
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        format!("{at}.{key}")
    } else {
        format!("{at}[{}]", serde_json::to_string(key).unwrap_or_default())
    }
}

/// A data file's text rewritten from `real`, the real answer (see the
/// module documentation); `None` when nothing changes.
pub fn update(text: &str, real: &Value) -> Result<Option<String>, String> {
    let old: Ordered = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let new = merge(&old, real);
    if new == old {
        return Ok(None);
    }
    let mut out = String::new();
    write_pretty(&new, 0, &mut out);
    out.push('\n');
    Ok(Some(out))
}

/// JSON that keeps its object keys in order and its numbers as written.
#[derive(Debug, Clone, PartialEq)]
enum Ordered {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl From<&Value> for Ordered {
    fn from(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => Self::Number(n.clone()),
            Value::String(s) => Self::String(s.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from).collect()),
            Value::Object(map) => Self::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::from(v)))
                    .collect(),
            ),
        }
    }
}

fn same_kind(old: &Ordered, real: &Value) -> bool {
    matches!(
        (old, real),
        (Ordered::Null, Value::Null)
            | (Ordered::Bool(_), Value::Bool(_))
            | (Ordered::Number(_), Value::Number(_))
            | (Ordered::String(_), Value::String(_))
    )
}

/// `old` with `real`'s shape: see [`update`].
fn merge(old: &Ordered, real: &Value) -> Ordered {
    match (old, real) {
        (Ordered::Object(fields), Value::Object(r)) => {
            let mut out: Vec<(String, Ordered)> = fields
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), merge(v, r.get(k)?))))
                .collect();
            for (k, v) in r {
                if !fields.iter().any(|(f, _)| f == k) {
                    out.push((k.clone(), Ordered::from(v)));
                }
            }
            Ordered::Object(out)
        }
        (Ordered::Array(items), Value::Array(r)) => match r.first() {
            // An empty real list says nothing about the shape of items.
            None => old.clone(),
            Some(first) if items.is_empty() => Ordered::Array(vec![Ordered::from(first)]),
            Some(first) => Ordered::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| merge(item, r.get(i).unwrap_or(first)))
                    .collect(),
            ),
        },
        (old, real) if same_kind(old, real) => old.clone(),
        (_, real) => Ordered::from(real),
    }
}

/// Two-space indented, as `serde_json::to_string_pretty` writes.
fn write_pretty(value: &Ordered, depth: usize, out: &mut String) {
    let pad = |depth: usize| "  ".repeat(depth);
    match value {
        Ordered::Null => out.push_str("null"),
        Ordered::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        Ordered::Number(n) => {
            let _ = write!(out, "{n}");
        }
        Ordered::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Ordered::Array(items) if items.is_empty() => out.push_str("[]"),
        Ordered::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                write_pretty(item, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push(']');
        }
        Ordered::Object(fields) if fields.is_empty() => out.push_str("{}"),
        Ordered::Object(fields) => {
            out.push_str("{\n");
            for (i, (k, v)) in fields.iter().enumerate() {
                out.push_str(&pad(depth + 1));
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push_str(": ");
                write_pretty(v, depth + 1, out);
                out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(depth));
            out.push('}');
        }
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(b))
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<Ordered, E> {
                Ok(Ordered::Number(n.into()))
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> Result<Ordered, E> {
                Ok(Ordered::Number(n.into()))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> Result<Ordered, E> {
                serde_json::Number::from_f64(n)
                    .map(Ordered::Number)
                    .ok_or_else(|| E::custom("not a JSON number"))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Ordered, E> {
                Ok(Ordered::String(s.to_owned()))
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<Ordered, E> {
                Ok(Ordered::String(s))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::Array(items))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut fields = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, Ordered>()? {
                    fields.push((k, v));
                }
                Ok(Ordered::Object(fields))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shapes_are_compared_not_values() {
        let real = json!([{ "id": 1, "name": "Keyboard", "price_cents": 4900, "tags": ["a"] }]);
        let mock = json!([{ "id": 7, "name": "Mouse", "price": 49, "tags": [] }]);
        assert_eq!(
            shape(&real, &mock),
            [
                r#"$[]: the real answer has field "price_cents" (number); the mock has "price" (number)"#
            ]
        );
        assert!(shape(&json!({ "a": 1.5 }), &json!({ "a": 2 })).is_empty());
        assert_eq!(
            shape(
                &json!({ "a": "x", "b": null, "c": 1, "d": { "e": true }, "f-g": 1 }),
                &json!({ "a": 1, "b": "y", "c": null, "d": { "e": "no" }, "h": [], "i": {}, "f-g": "2" })
            ),
            [
                r#"$: the mock has field "h" (array); the real answer has none"#,
                r#"$: the mock has field "i" (object); the real answer has none"#,
                "$.a: the real answer has a string; the mock has a number",
                "$.b: the real answer has null; the mock has a string",
                "$.c: the real answer has a number; the mock has null",
                "$.d.e: the real answer has a boolean; the mock has a string",
                r#"$["f-g"]: the real answer has a number; the mock has a string"#,
            ]
        );
    }

    #[test]
    fn updates_keep_order_and_values_that_fit() {
        let text = "[\n  { \"id\": 1, \"name\": \"Keyboard\", \"price\": 49, \"sale\": true },\n  { \"id\": 2, \"name\": \"Mouse\", \"price\": 19, \"sale\": false }\n]\n";
        let real = json!([
            { "id": 10, "name": "Real", "price_cents": 4900, "sale": "yes", "stock": null },
            { "id": 11, "name": "Real 2", "price_cents": 1900, "sale": "no", "stock": 3 }
        ]);
        let updated = update(text, &real).unwrap().unwrap();
        assert_eq!(
            updated,
            r#"[
  {
    "id": 1,
    "name": "Keyboard",
    "sale": "yes",
    "price_cents": 4900,
    "stock": null
  },
  {
    "id": 2,
    "name": "Mouse",
    "sale": "no",
    "price_cents": 1900,
    "stock": 3
  }
]
"#
        );
        // Unchanged shape: nothing to write.
        assert_eq!(update("{\"a\": 1}", &json!({ "a": 2 })).unwrap(), None);
        // An empty real list keeps the file's items.
        assert_eq!(update("[{\"a\": 1}]", &json!([])).unwrap(), None);
        assert!(update("not json", &json!(1)).is_err());
    }
}
