//! `expect mock-state <name> <check>` (plan 11, milestone 11.3): a named
//! value or collection of the mock sets in use. It needs no page: it
//! looks at the session's mocks again every [`POLL`] until the check
//! holds or the timeout passes, since the page changes them by sending
//! requests.

use std::time::Duration;

use mock::StateItem;
use protocol::schema::ExpectData;
use protocol::{Cmd, ErrorInfo, ErrorKind, Hint, Output, format};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{assertion_text, expected};
use crate::Engine;
use crate::context::Scoped as _;
use crate::request::{ExpectRequest, MockStateCheck, millis};
use crate::route::unknown_state;

/// How often the state is looked at again.
const POLL: Duration = Duration::from_millis(25);

/// What a check saw.
struct Seen {
    holds: bool,
    /// For `--json`: a count, or the value.
    actual: Value,
    /// In words: `3 items`, `true`.
    text: String,
}

/// Checks `check` on `item`; a check that cannot apply to it is `Err`.
fn evaluate(name: &str, check: &MockStateCheck, item: &StateItem) -> Result<Seen, ErrorInfo> {
    let value = match item {
        StateItem::Value(value) => value.clone(),
        StateItem::Collection { items, .. } => Value::Array(items.clone()),
    };
    let not_a_list = |what: &str| {
        ErrorInfo::new(
            ErrorKind::BadArgs,
            format!(
                "expect mock-state {name} {what}: {name} is not a collection or a list; it is {}",
                mock::text::compact(&value)
            ),
        )
        .hint(Hint::run(
            Cmd::new("expect")
                .arg("mock-state")
                .arg(name)
                .raw("value")
                .arg(value.to_string()),
        ))
    };
    Ok(match check {
        MockStateCheck::Count { count, min, max } => {
            let Some(items) = value.as_array() else {
                return Err(not_a_list("count"));
            };
            let n = items.len() as u64;
            let holds = count.is_none_or(|c| n == c)
                && min.is_none_or(|m| n >= m)
                && max.is_none_or(|m| n <= m);
            Seen {
                holds,
                actual: json!(n),
                text: format!("{n} {}", if n == 1 { "item" } else { "items" }),
            }
        }
        MockStateCheck::Value { value: want } => Seen {
            holds: same(want, &value),
            text: mock::text::compact(&value),
            actual: value,
        },
        MockStateCheck::Contains { value: want } => match &value {
            Value::Array(items) => {
                let found = items.iter().any(|item| mock::json_contains(want, item));
                Seen {
                    holds: found,
                    text: if found {
                        "an item matches".into()
                    } else {
                        format!(
                            "none of its {} {} matches",
                            items.len(),
                            if items.len() == 1 { "item" } else { "items" }
                        )
                    },
                    actual: value.clone(),
                }
            }
            Value::Object(_) => Seen {
                holds: mock::json_contains(want, &value),
                text: mock::text::compact(&value),
                actual: value.clone(),
            },
            _ => return Err(not_a_list("contains")),
        },
    })
}

/// JSON equality, with numbers compared by value (1 is 1.0).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

impl Engine {
    /// `expect mock-state`: retried until it holds or the timeout passes.
    pub(super) async fn expect_mock_state(
        &self,
        req: &ExpectRequest,
        name: &str,
        check: &MockStateCheck,
    ) -> Result<Output, ErrorInfo> {
        let start = Instant::now();
        let timeout = millis(req.timeout_ms);
        let deadline = start + timeout;
        let mut tries: u64 = 0;
        let scope = self.context_key()?;
        loop {
            let item = self.settings.routes.with_mocks(scope.as_deref(), |mocks| {
                if mocks.is_empty() {
                    return Err(ErrorInfo::new(
                        ErrorKind::NotFound,
                        "no mock sets in use, so there is no mock state to check",
                    )
                    .hint(Hint::run(
                        Cmd::new("mock")
                            .arg("use")
                            .raw("<set>")
                            .scoped(&self.session),
                    )));
                }
                mocks
                    .lookup(name)
                    .ok_or_else(|| unknown_state(&self.session, name, &mocks.state_names()))
            })?;
            let seen = evaluate(name, check, &item)?;
            tries += 1;
            if seen.holds != req.not {
                let elapsed = start.elapsed();
                let text = format!(
                    "passed: {} ({})",
                    assertion_text(req),
                    format::duration(elapsed)
                );
                let count = match &item {
                    StateItem::Collection { items, .. } => Some(items.len() as u64),
                    StateItem::Value(_) => None,
                };
                let data = ExpectData {
                    tab: String::new(),
                    assertion: assertion_text(req),
                    expected: expected(req, None),
                    actual: seen.actual,
                    count,
                    elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    retries: tries - 1,
                    layout: None,
                };
                return Ok(Output::typed(&data, text)
                    .quiet(seen.text)
                    .limit(req.max_chars, "--max-chars 0 for all of it"));
            }
            if Instant::now() >= deadline {
                let retries = tries - 1;
                let actual = format!(
                    "{} (after {}, {retries} {})",
                    seen.text,
                    format::duration(timeout),
                    if retries == 1 { "retry" } else { "retries" }
                );
                let expected = expected(req, None);
                return Err(ErrorInfo::new(
                    ErrorKind::ExpectFailed,
                    format!("expect failed: {}", assertion_text(req)),
                )
                .note("expected", &expected)
                .note("actual", actual)
                .hint(
                    Hint::run(
                        Cmd::new("mock")
                            .arg("state")
                            .arg(name)
                            .scoped(&self.session),
                    )
                    .why("shows it all")
                    .or(format!(
                        "{} shows which mock answered each request",
                        Cmd::new("mock").arg("log").scoped(&self.session)
                    )),
                )
                .details(json!({
                    "assertion": assertion_text(req),
                    "expected": expected,
                    "actual": seen.actual,
                    "retries": retries,
                    "timeout_ms": req.timeout_ms,
                })));
            }
            tokio::time::sleep(POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
        }
    }
}
