//! The text forms of the `mock` commands' output, shared by the daemon and
//! the CLI (which explains without a session).

use std::fmt::Write as _;

use protocol::schema::{
    MockCollectionInfo, MockExplainData, MockInfo, MockSetInfo, MockStateData, RoutedRequest,
};
use serde_json::Value;

/// How much of a URL a line shows.
const SHOWN_URL: usize = 150;

/// `base  5 mocks  scope /api/** (unmatched: fail)  /abs/e2e/mocks/base.toml`.
pub fn set_line(set: &MockSetInfo) -> String {
    let mut line = format!(
        "{}  {} {}",
        set.name,
        set.count,
        if set.count == 1 { "mock" } else { "mocks" }
    );
    if set.collections > 0 {
        let _ = write!(
            line,
            ", {} {}",
            set.collections,
            if set.collections == 1 {
                "collection"
            } else {
                "collections"
            }
        );
    }
    if !set.scope.is_empty() {
        let _ = write!(
            line,
            "  scope {} (unmatched: {})",
            set.scope.join(", "),
            set.unmatched
        );
    }
    if set.cors != "auto" {
        let _ = write!(line, "  cors {}", set.cors);
    }
    if set.latency != "0ms" {
        let _ = write!(line, "  latency {}", set.latency);
    }
    let _ = write!(line, "  {}", set.file);
    if let Some(error) = &set.reload_error {
        let _ = write!(
            line,
            "\n  reload failed, the previous version is still in use: {error}"
        );
    }
    line
}

/// `base.toml:5 "list products" GET /api/products → fulfill 200 json_file
/// data/products.json (3 hits)`.
pub fn mock_line(mock: &MockInfo) -> String {
    format!(
        "{} \"{}\" {} → {} ({} {})",
        mock.mock.at,
        mock.mock.name,
        mock.matches,
        mock.responds,
        mock.hits,
        if mock.hits == 1 { "hit" } else { "hits" }
    )
}

/// The sets in the order they are tried, each with its mocks if it has
/// them listed.
pub fn sets(sets: &[MockSetInfo]) -> String {
    let mut text = String::new();
    for set in sets {
        let _ = writeln!(text, "{}", set_line(set));
        for mock in &set.mocks {
            let _ = writeln!(text, "  {}", mock_line(mock));
        }
    }
    if sets.len() > 1 {
        text.push_str("(tried top to bottom, after any `route` rules)\n");
    }
    text.trim_end().to_owned()
}

/// `#12 t1 GET http://x/api/products xhr → base.toml:5 "list products"
/// fulfill 200`.
pub fn log_line(entry: &RoutedRequest) -> String {
    let url: String = entry.url.chars().take(SHOWN_URL).collect();
    let more = if url.len() < entry.url.len() {
        "…"
    } else {
        ""
    };
    let mut line = format!("#{} ", entry.seq);
    if let Some(tab) = &entry.tab {
        let _ = write!(line, "{tab} ");
    }
    let _ = write!(
        line,
        "{} {url}{more} {} → ",
        entry.method, entry.resource_type
    );
    if let Some(route) = &entry.route {
        let _ = write!(line, "{route} ");
    } else if let Some(mock) = &entry.mock {
        let _ = write!(line, "{} \"{}\" ", mock.at, mock.name);
    }
    line.push_str(&entry.outcome);
    if let Some(set) = &entry.unmatched {
        let _ = write!(line, " (scope of {set})");
    }
    line
}

/// `mock explain`: the answer, then every mock checked (at most `limit`,
/// 0 for all).
pub fn explain(data: &MockExplainData, limit: usize) -> String {
    let mut text = format!("{} {}\n", data.method, data.url);
    if let Some(route) = &data.route {
        let _ = writeln!(
            text,
            "→ route {route} answers it first (rules come before mocks; `routes` lists them)"
        );
    }
    match &data.answer {
        Some(mock) => {
            let _ = writeln!(text, "→ {} \"{}\": {}", mock.at, mock.name, data.outcome);
        }
        None => {
            let _ = writeln!(text, "→ no mock matches; {}", data.outcome);
        }
    }
    if !data.params.is_empty() {
        let params: Vec<String> = data
            .params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(text, "  params: {}", params.join(", "));
    }
    if data.checked.is_empty() {
        text.push_str("no mock sets in use");
        return text;
    }
    text.push_str("mocks, in the order they are tried:\n");
    let shown = if limit == 0 {
        data.checked.len()
    } else {
        limit.min(data.checked.len())
    };
    for check in &data.checked[..shown] {
        let mark = if check.matches { "✓" } else { "✗" };
        let _ = write!(text, "  {mark} {} \"{}\"", check.mock.at, check.mock.name);
        if let Some(reason) = &check.reason {
            let _ = write!(text, ": {reason}");
        }
        text.push('\n');
    }
    if shown < data.checked.len() {
        let _ = writeln!(
            text,
            "  … {} more; --limit 0 shows them all",
            data.checked.len() - shown
        );
    }
    text.trim_end().to_owned()
}

/// `todos  4 items  /api/todos  (base.toml:30)`.
pub fn collection_line(c: &MockCollectionInfo) -> String {
    format!(
        "{}  {} {}  {}  ({})",
        c.mock.name,
        c.count,
        if c.count == 1 { "item" } else { "items" },
        c.path,
        c.mock.at
    )
}

/// `mock state`: the named values (`name = value`), then the
/// collections; with a NAME, its value or items as indented JSON.
pub fn state(data: &MockStateData) -> String {
    if let Some(value) = &data.value {
        return serde_json::to_string_pretty(value).unwrap_or_default();
    }
    let mut text = String::new();
    for (name, value) in &data.values {
        let _ = writeln!(text, "{name} = {}", compact(value));
    }
    if !data.collections.is_empty() {
        if !data.values.is_empty() {
            text.push_str("collections:\n");
        }
        for c in &data.collections {
            let _ = writeln!(text, "{}", collection_line(c));
        }
    }
    text.trim_end().to_owned()
}

/// A value on one line.
pub fn compact(value: &Value) -> String {
    let text = value.to_string();
    let shown: String = text.chars().take(SHOWN_URL).collect();
    if shown.len() < text.len() {
        format!("{shown}…")
    } else {
        shown
    }
}
