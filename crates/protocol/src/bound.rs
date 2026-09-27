//! Keeping output bounded (plan 08 §2).
//!
//! Text is cut at a character budget and ends with one line saying how to
//! see more. `--json` output must stay valid JSON, so it is never cut
//! mid-document. Instead the longest strings are shortened (ending in `…`)
//! and, if that is not enough, the longest arrays lose their tail, until
//! `data` fits the same budget. Each cut is listed in the envelope's
//! `truncated` array and summed up in a warning.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::format::thousands;

/// Strings are cut to at least this many characters before any array is
/// shortened, so the shape of a list survives a few long values.
const MIN_STRING: usize = 200;

/// Marks a shortened string.
const ELLIPSIS: char = '…';

/// A command's output budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    /// The most characters of text, or of `data` serialised as JSON.
    /// `0` means no limit.
    pub max_chars: usize,
    /// How to see more, after "use": `--max-chars 0 for all of it`.
    pub more: String,
}

impl Limit {
    /// A budget of `max_chars` (0 = none) and the way to see more.
    pub fn new(max_chars: usize, more: impl Into<String>) -> Self {
        Self {
            max_chars,
            more: more.into(),
        }
    }
}

/// One value that `--json` output shortened to fit the budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Truncation {
    /// Where, as a JSON Pointer into `data` (`/result/value`).
    pub path: String,
    /// A string (lengths in characters) or an array (lengths in items).
    pub kind: TruncatedKind,
    /// The original length.
    pub len: usize,
    /// How much was kept (a cut string also ends in `…`).
    pub kept: usize,
}

/// What kind of value was shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TruncatedKind {
    /// Lengths count characters.
    String,
    /// Lengths count items.
    Array,
}

/// Cuts `text` after `max_chars` characters (0 = no limit). Returns the kept
/// part and how many characters were dropped, or `None` if it fits.
pub fn cut(text: &str, max_chars: usize) -> Option<(&str, usize)> {
    if max_chars == 0 {
        return None;
    }
    let (at, _) = text.char_indices().nth(max_chars)?;
    Some((&text[..at], text[at..].chars().count()))
}

/// The last line of cut text: `… truncated 18,203 chars; use <more>`.
pub fn truncated_line(dropped: usize, more: &str) -> String {
    format!("… truncated {} chars; use {more}", thousands(dropped))
}

/// Cuts `text` to at most `max_chars` characters (0 = no limit). A cut text
/// ends with one line saying how much was dropped and how to see it.
pub fn truncate(text: &str, max_chars: usize, more: &str) -> String {
    match cut(text, max_chars) {
        None => text.to_owned(),
        Some((kept, dropped)) => format!("{kept}\n{}", truncated_line(dropped, more)),
    }
}

/// Shortens `data` until it serialises to at most `max_chars` characters
/// (0 = no limit) and returns what was cut, longest-first by path order.
///
/// It first caps every string at the largest length that fits. If strings
/// of [`MIN_STRING`] characters still do not fit, it caps every array at
/// the largest length that fits and then lets strings grow back into the
/// room left. If even empty arrays and strings do not fit (an object with
/// huge numbers of keys), the result is as small as it can get but over
/// budget.
pub fn bound_json(data: &mut Value, max_chars: usize) -> Vec<Truncation> {
    if max_chars == 0 || json_len(data) <= max_chars {
        return Vec::new();
    }
    // Nothing longer than the budget can fit, so search on a copy capped
    // at the budget: the same answer, without reserialising megabytes of
    // screenshot per step. The cuts are then applied to the original.
    let small = capped(data, Caps::both(max_chars, max_chars), "", &mut None);
    let (longest_string, longest_array) = longest(&small);
    let fits = |caps: Caps| json_len(&capped(&small, caps, "", &mut None)) <= max_chars;

    let caps = if fits(Caps::strings(MIN_STRING.min(longest_string))) {
        let s = last_fitting(MIN_STRING, longest_string, |s| fits(Caps::strings(s)));
        Caps::strings(s)
    } else {
        let s = MIN_STRING.min(longest_string);
        if fits(Caps::both(s, 0)) {
            let a = last_fitting(0, longest_array, |a| fits(Caps::both(s, a)));
            let s = last_fitting(s, longest_string, |s| fits(Caps::both(s, a)));
            Caps::both(s, a)
        } else {
            let s = last_fitting(0, s, |s| fits(Caps::both(s, 0)));
            Caps::both(s, 0)
        }
    };
    let mut cuts = Some(Vec::new());
    *data = capped(data, caps, "", &mut cuts);
    cuts.unwrap_or_default()
}

/// The warning for `--json` output that [`bound_json`] shortened.
pub fn truncated_warning(cuts: &[Truncation], limit: &Limit) -> String {
    const SHOWN: usize = 3;
    let mut out = format!("data truncated to {} chars:", thousands(limit.max_chars));
    for (i, cut) in cuts.iter().take(SHOWN).enumerate() {
        let unit = match cut.kind {
            TruncatedKind::String => "chars",
            TruncatedKind::Array => "items",
        };
        let sep = if i == 0 { " " } else { ", " };
        let path = if cut.path.is_empty() { "/" } else { &cut.path };
        let _ = write!(
            out,
            "{sep}{path} {} → {} {unit}",
            thousands(cut.len),
            thousands(cut.kept)
        );
    }
    if cuts.len() > SHOWN {
        let _ = write!(out, ", and {} more", thousands(cuts.len() - SHOWN));
    }
    let _ = write!(out, "; use {}", limit.more);
    out
}

/// Caps on string length (characters) and array length (items).
#[derive(Debug, Clone, Copy)]
struct Caps {
    string: usize,
    array: usize,
}

impl Caps {
    fn strings(string: usize) -> Self {
        Self {
            string,
            array: usize::MAX,
        }
    }

    fn both(string: usize, array: usize) -> Self {
        Self { string, array }
    }
}

/// The largest `n` in `lo..=hi` for which `fits(n)` holds, assuming `fits`
/// is monotonic and `fits(lo)` holds (else `lo`).
fn last_fitting(lo: usize, hi: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (lo, hi.max(lo));
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

fn json_len(value: &Value) -> usize {
    value.to_string().chars().count()
}

/// The longest string (characters) and array (items) anywhere in `value`.
fn longest(value: &Value) -> (usize, usize) {
    match value {
        Value::String(s) => (s.chars().count(), 0),
        Value::Array(items) => items
            .iter()
            .map(longest)
            .fold((0, items.len()), |(s, a), (s2, a2)| (s.max(s2), a.max(a2))),
        Value::Object(map) => map
            .values()
            .map(longest)
            .fold((0, 0), |(s, a), (s2, a2)| (s.max(s2), a.max(a2))),
        Value::Null | Value::Bool(_) | Value::Number(_) => (0, 0),
    }
}

/// A copy of `value` with `caps` applied, recording cuts if asked.
fn capped(value: &Value, caps: Caps, path: &str, cuts: &mut Option<Vec<Truncation>>) -> Value {
    match value {
        Value::String(s) => match cut(s, caps.string) {
            Some((kept, _)) => {
                record(
                    cuts,
                    path,
                    TruncatedKind::String,
                    s.chars().count(),
                    caps.string,
                );
                Value::String(format!("{kept}{ELLIPSIS}"))
            }
            // `cut` treats 0 as "no limit"; here it means "nothing left".
            None if caps.string == 0 && !s.is_empty() => {
                record(cuts, path, TruncatedKind::String, s.chars().count(), 0);
                Value::String(ELLIPSIS.to_string())
            }
            None => value.clone(),
        },
        Value::Array(items) => {
            if items.len() > caps.array {
                record(cuts, path, TruncatedKind::Array, items.len(), caps.array);
            }
            Value::Array(
                items
                    .iter()
                    .take(caps.array)
                    .enumerate()
                    .map(|(i, item)| capped(item, caps, &format!("{path}/{i}"), cuts))
                    .collect(),
            )
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    let path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                    (key.clone(), capped(item, caps, &path, cuts))
                })
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

fn record(
    cuts: &mut Option<Vec<Truncation>>,
    path: &str,
    kind: TruncatedKind,
    len: usize,
    kept: usize,
) {
    if let Some(cuts) = cuts {
        cuts.push(Truncation {
            path: path.to_owned(),
            kind,
            len,
            kept,
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        Limit, TruncatedKind, Truncation, bound_json, json_len, truncate, truncated_warning,
    };

    #[test]
    fn truncate_keeps_short_text() {
        assert_eq!(truncate("hello", 5, "--max-chars 10"), "hello");
        assert_eq!(truncate("hello", 0, "--max-chars 10"), "hello");
    }

    #[test]
    fn truncate_cuts_on_char_boundaries_and_says_how_to_see_more() {
        let text = "ééééé".to_owned() + &"x".repeat(1234);
        assert_eq!(
            truncate(&text, 3, "--max-chars 0"),
            "ééé\n… truncated 1,236 chars; use --max-chars 0"
        );
    }

    #[test]
    fn small_json_is_untouched() {
        let mut data = json!({"a": "x".repeat(100), "b": [1, 2, 3]});
        let before = data.clone();
        assert!(bound_json(&mut data, 1000).is_empty());
        assert!(bound_json(&mut data, 0).is_empty());
        assert_eq!(data, before);
    }

    #[test]
    fn one_long_string_is_cut_to_fit() {
        let mut data = json!({"result": {"type": "string", "value": "x".repeat(100_000)}});
        let cuts = bound_json(&mut data, 20_000);
        assert!(json_len(&data) <= 20_000);
        assert!(
            json_len(&data) > 19_900,
            "uses the budget: {}",
            json_len(&data)
        );
        let value = data["result"]["value"].as_str().unwrap();
        assert!(value.ends_with('…'));
        assert_eq!(data["result"]["type"], json!("string"));
        assert_eq!(
            cuts,
            vec![Truncation {
                path: "/result/value".into(),
                kind: TruncatedKind::String,
                len: 100_000,
                kept: value.chars().count() - 1,
            }]
        );
        let warning = truncated_warning(&cuts, &Limit::new(20_000, "--max-chars 0"));
        assert!(
            warning.starts_with("data truncated to 20,000 chars: /result/value 100,000 → 19,9"),
            "{warning}"
        );
        assert!(warning.ends_with(" chars; use --max-chars 0"), "{warning}");
    }

    #[test]
    fn long_lists_lose_their_tail_but_keep_their_items() {
        let items: Vec<_> = (0..1000)
            .map(|i| json!({"id": i, "name": format!("item {i}"), "body": "y".repeat(500)}))
            .collect();
        let mut data = json!({"items": items});
        let cuts = bound_json(&mut data, 20_000);
        assert!(json_len(&data) <= 20_000);
        let kept = data["items"].as_array().unwrap();
        assert!(kept.len() > 10, "{}", kept.len());
        assert_eq!(kept[0]["name"], json!("item 0"));
        assert!(kept[0]["body"].as_str().unwrap().chars().count() >= 200);
        assert_eq!(cuts[0].path, "/items");
        assert_eq!(cuts[0].kind, TruncatedKind::Array);
        assert_eq!(cuts[0].len, 1000);
        let warning = truncated_warning(&cuts, &Limit::new(20_000, "--limit 1000"));
        assert!(warning.contains("/items 1,000 → "), "{warning}");
        assert!(warning.contains(" more; use --limit 1000"), "{warning}");
    }

    #[test]
    fn huge_values_are_cut_as_if_searched_in_full() {
        let mut data = json!({"data": "q".repeat(3_000_000), "list": vec![1; 50_000]});
        let cuts = bound_json(&mut data, 1000);
        assert!(json_len(&data) <= 1000);
        assert_eq!(cuts[0].path, "/data");
        assert_eq!(cuts[0].len, 3_000_000);
        assert_eq!(cuts[1].path, "/list");
        assert_eq!(cuts[1].len, 50_000);
        assert_eq!(data["list"].as_array().unwrap().len(), cuts[1].kept);
    }

    #[test]
    fn pointer_segments_are_escaped() {
        let mut data = json!({"a/b": {"c~d": "z".repeat(1000)}});
        let cuts = bound_json(&mut data, 100);
        assert_eq!(cuts[0].path, "/a~1b/c~0d");
    }

    #[test]
    fn an_unfittable_object_ends_up_as_small_as_possible() {
        let map: serde_json::Map<_, _> = (0..100)
            .map(|i| (format!("key{i}"), json!("v".repeat(50))))
            .collect();
        let mut data = serde_json::Value::Object(map);
        let cuts = bound_json(&mut data, 50);
        assert_eq!(cuts.len(), 100);
        assert_eq!(data["key0"], json!("…"));
    }
}
