//! `mock record --match-bodies`: the **stable** fields of recorded request
//! bodies, which tell requests to one path apart (two logins as different
//! users) without the fields a client makes up each time (ids, the time,
//! tokens).
//!
//! A field is left out when its value differs between the recorded
//! requests of one method and path *and* every value looks generated: a
//! UUID, a number or date close to when the request was sent, or a
//! string over 64 characters. The rest is kept.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::multipart;
use crate::record::Exchange;

/// Strings longer than this look generated (tokens, signatures).
const LONG: usize = 64;

/// How close to a request's time a number or date must be to look like
/// the time it was sent.
const NEAR_MS: i64 = 2 * 24 * 3600 * 1000;

/// What a recorded request's body is matched as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Kind {
    /// `json = { … }`.
    Json,
    /// `form = { … }` (urlencoded, or a multipart body's text fields).
    Form,
    /// `graphql = { variables = { … } }`.
    Graphql,
}

/// A request body's fields: a JSON object (a form's values are strings).
#[derive(Debug, Clone)]
pub(crate) struct Fields {
    pub(crate) kind: Kind,
    pub(crate) value: Map<String, Value>,
    /// When the request was sent (ms since the epoch).
    pub(crate) at_ms: i64,
}

/// The fields of `ex`'s body, if it has some a mock can match: a JSON
/// object, a form, or (for a GraphQL request) its variables.
pub(crate) fn fields(ex: &Exchange, graphql: bool, now_ms: i64) -> Option<Fields> {
    let body = ex.request_body.as_deref()?;
    let at_ms = ex.started_ms.unwrap_or(now_ms);
    let content_type = ex
        .request_headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    let (kind, value) = if graphql {
        let json: Value = serde_json::from_str(body).ok()?;
        (Kind::Graphql, json.get("variables")?.as_object()?.clone())
    } else if let Some(boundary) = multipart::boundary(content_type) {
        let parts = multipart::parse(body.as_bytes(), &boundary.ok()?).ok()?;
        let mut map = Map::new();
        for part in parts.into_iter().filter(|p| p.file.is_none()) {
            map.entry(part.name).or_insert(Value::String(part.value));
        }
        (Kind::Form, map)
    } else if content_type
        .to_ascii_lowercase()
        .starts_with("application/x-www-form-urlencoded")
    {
        let mut map = Map::new();
        for (k, v) in url::form_urlencoded::parse(body.as_bytes()) {
            map.entry(k.into_owned())
                .or_insert(Value::String(v.into_owned()));
        }
        (Kind::Form, map)
    } else {
        match serde_json::from_str::<Value>(body).ok()? {
            Value::Object(map) => (Kind::Json, map),
            _ => return None,
        }
    };
    (!value.is_empty()).then_some(Fields { kind, value, at_ms })
}

/// A place in a body: the object keys down to a value.
type Pointer = Vec<String>;

/// The values of `map`, by where they are; objects are walked into,
/// anything else (arrays too) is one value.
fn leaves<'a>(
    map: &'a Map<String, Value>,
    at: &mut Pointer,
    out: &mut BTreeMap<Pointer, &'a Value>,
) {
    for (key, value) in map {
        at.push(key.clone());
        match value {
            Value::Object(inner) if !inner.is_empty() => leaves(inner, at, out),
            _ => {
                out.insert(at.clone(), value);
            }
        }
        at.pop();
    }
}

/// The places to leave out of the bodies of one method and path: those
/// whose values differ between the bodies and all look generated.
pub(crate) fn volatile(bodies: &[&Fields]) -> BTreeSet<Pointer> {
    let all: Vec<BTreeMap<Pointer, &Value>> = bodies
        .iter()
        .map(|f| {
            let mut out = BTreeMap::new();
            leaves(&f.value, &mut Vec::new(), &mut out);
            out
        })
        .collect();
    let places: BTreeSet<&Pointer> = all.iter().flat_map(BTreeMap::keys).collect();
    places
        .into_iter()
        .filter(|place| {
            let values: Vec<(Option<&&Value>, i64)> = all
                .iter()
                .zip(bodies)
                .map(|(leaves, f)| (leaves.get(*place), f.at_ms))
                .collect();
            let differ = values.windows(2).any(|w| w[0].0 != w[1].0);
            differ
                && values
                    .iter()
                    .all(|(v, at)| v.is_none_or(|v| looks_generated(v, *at)))
        })
        .cloned()
        .collect()
}

/// `fields` without the places in `drop`, and without `null`s (a mock
/// file cannot say them).
pub(crate) fn stable(fields: &Map<String, Value>, drop: &BTreeSet<Pointer>) -> Map<String, Value> {
    fn walk(
        map: &Map<String, Value>,
        at: &mut Pointer,
        drop: &BTreeSet<Pointer>,
    ) -> Map<String, Value> {
        let mut out = Map::new();
        for (key, value) in map {
            at.push(key.clone());
            if !drop.contains(at) && !value.is_null() {
                match value {
                    Value::Object(inner) if !inner.is_empty() => {
                        let inner = walk(inner, at, drop);
                        if !inner.is_empty() {
                            out.insert(key.clone(), Value::Object(inner));
                        }
                    }
                    _ => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            at.pop();
        }
        out
    }
    walk(fields, &mut Vec::new(), drop)
}

/// The number of values in `map` (objects walked into).
pub(crate) fn count(map: &Map<String, Value>) -> usize {
    let mut out = BTreeMap::new();
    leaves(map, &mut Vec::new(), &mut out);
    out.len()
}

/// `map` as text that is equal for equal maps, whatever their key order.
pub(crate) fn canonical(map: &Map<String, Value>) -> String {
    fn sorted(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let sorted_map: BTreeMap<&String, Value> =
                    map.iter().map(|(k, v)| (k, sorted(v))).collect();
                Value::Object(
                    sorted_map
                        .into_iter()
                        .map(|(k, v)| (k.clone(), v))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    sorted(&Value::Object(map.clone())).to_string()
}

/// The values that differ between `maps`, by where they are: at most
/// `n`, as `email=ada@example.com` for the map at `i`.
pub(crate) fn telling(maps: &[&Map<String, Value>], i: usize, n: usize) -> Vec<String> {
    let all: Vec<BTreeMap<Pointer, &Value>> = maps
        .iter()
        .map(|m| {
            let mut out = BTreeMap::new();
            leaves(m, &mut Vec::new(), &mut out);
            out
        })
        .collect();
    let mut out = Vec::new();
    for (place, value) in &all[i] {
        if out.len() == n {
            break;
        }
        if all.iter().any(|other| other.get(place) != Some(value)) {
            let shown = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let shown: String = shown.chars().take(24).collect();
            out.push(format!("{}={shown}", place.join(".")));
        }
    }
    out
}

/// Whether a value looks made up by the client for one request, sent at
/// `at_ms`.
fn looks_generated(value: &Value, at_ms: i64) -> bool {
    match value {
        Value::String(s) => {
            s.chars().count() > LONG
                || is_uuid(s)
                || date_ms(s).is_some_and(|t| near(t, at_ms))
                || s.parse::<f64>().is_ok_and(|n| near_number(n, at_ms))
        }
        Value::Number(n) => n.as_f64().is_some_and(|n| near_number(n, at_ms)),
        _ => false,
    }
}

fn near(t: i64, at_ms: i64) -> bool {
    (t - at_ms).abs() <= NEAR_MS
}

/// A Unix time in seconds or milliseconds near `at_ms`.
#[allow(clippy::cast_possible_truncation)]
fn near_number(n: f64, at_ms: i64) -> bool {
    if !n.is_finite() || n <= 0.0 {
        return false;
    }
    near((n * 1000.0) as i64, at_ms) || near(n as i64, at_ms)
}

fn is_uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `2026-10-01`, `2026-10-01T12:00:00Z`, `2026-10-01T12:00:00.123+02:00`
/// in ms since the epoch.
#[allow(clippy::many_single_char_names)]
pub(crate) fn date_ms(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let s = text.get(from..from + len)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut ms = days_from_civil(y, m, d) * 86_400_000;
    if b.len() == 10 {
        return Some(ms);
    }
    if !matches!(b[10], b'T' | b't' | b' ') || b.len() < 19 || b[13] != b':' || b[16] != b':' {
        return None;
    }
    ms += num(11, 2)? * 3_600_000 + num(14, 2)? * 60_000 + num(17, 2)? * 1000;
    let mut rest = &text[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        let shown: String = frac[..digits]
            .chars()
            .chain("000".chars())
            .take(3)
            .collect();
        ms += shown.parse::<i64>().ok()?;
        rest = &frac[digits..];
    }
    match rest {
        "" | "Z" | "z" => Some(ms),
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let off = rest[1..].replace(':', "");
            if off.len() != 4 || !off.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let h: i64 = off[..2].parse().ok()?;
            let m: i64 = off[2..].parse().ok()?;
            Some(ms - sign * (h * 3_600_000 + m * 60_000))
        }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const AT: i64 = 1_790_000_000_000;

    #[allow(clippy::needless_pass_by_value)]
    fn fields(value: Value) -> Fields {
        Fields {
            kind: Kind::Json,
            value: value.as_object().unwrap().clone(),
            at_ms: AT,
        }
    }

    fn dropped(bodies: &[Value]) -> Vec<String> {
        let f: Vec<Fields> = bodies.iter().cloned().map(fields).collect();
        let refs: Vec<&Fields> = f.iter().collect();
        volatile(&refs).into_iter().map(|p| p.join(".")).collect()
    }

    #[test]
    fn dates() {
        assert_eq!(date_ms("1970-01-01"), Some(0));
        assert_eq!(date_ms("1970-01-02T00:00:01Z"), Some(86_401_000));
        assert_eq!(date_ms("1970-01-01T01:00:00.5+01:00"), Some(500));
        assert_eq!(date_ms("2026-10-01T00:00:00Z"), Some(1_790_812_800_000));
        assert_eq!(date_ms("2026-13-01"), None);
        assert_eq!(date_ms("not a date"), None);
    }

    #[test]
    fn generated_values_that_differ_are_dropped() {
        let near = AT + 1000;
        let bodies = [
            json!({
                "email": "ada@example.com",
                "requestId": "6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e5f",
                "sentAt": AT,
                "ts": "2026-09-21T09:33:20Z",
                "token": "x".repeat(80),
                "meta": {"client": "web", "nonce": "a1b2c3d4-0000-4000-8000-000000000001"},
                "fixed": "7d1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e5f",
                "qty": 1,
            }),
            json!({
                "email": "bob@example.com",
                "requestId": "0b9c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e5f",
                "sentAt": near,
                "ts": "2026-09-21T09:33:21Z",
                "token": "y".repeat(80),
                "meta": {"client": "web", "nonce": "a1b2c3d4-0000-4000-8000-000000000002"},
                "fixed": "7d1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e5f",
                "qty": 2,
            }),
        ];
        // The email and qty differ but look chosen; the fixed UUID does not
        // differ.
        assert_eq!(
            dropped(&bodies),
            ["meta.nonce", "requestId", "sentAt", "token", "ts"]
        );
        // A date far from the request differs, and is kept.
        assert!(
            dropped(&[json!({"born": "1990-01-01"}), json!({"born": "1991-02-03"})]).is_empty()
        );
        // One request: nothing differs, nothing is dropped.
        assert!(dropped(&bodies[..1]).is_empty());
        // A field only some bodies have is dropped if it looks generated.
        assert_eq!(
            dropped(&[
                json!({"a": 1}),
                json!({"a": 1, "id": "6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e5f"})
            ]),
            ["id"]
        );
    }

    #[test]
    fn stable_subsets_and_their_keys() {
        let body = json!({"b": 1, "a": {"x": null, "y": "6f1c2a5e-1d2b-4c3d-8e9f-0a1b2c3d4e5f"}, "c": null});
        let map = body.as_object().unwrap();
        let drop = BTreeSet::from([vec!["a".to_owned(), "y".to_owned()]]);
        let kept = stable(map, &drop);
        assert_eq!(Value::Object(kept.clone()), json!({"b": 1}));
        assert_eq!(count(map), 4);
        let other = json!({"a": {"y": 2, "x": 1}, "b": 1});
        let same = json!({"b": 1, "a": {"x": 1, "y": 2}});
        assert_eq!(
            canonical(other.as_object().unwrap()),
            canonical(same.as_object().unwrap())
        );
        let a = json!({"email": "ada@example.com", "remember": true});
        let b = json!({"email": "bob@example.com", "remember": true});
        let maps = [a.as_object().unwrap(), b.as_object().unwrap()];
        assert_eq!(telling(&maps, 1, 2), ["email=bob@example.com"]);
    }
}
