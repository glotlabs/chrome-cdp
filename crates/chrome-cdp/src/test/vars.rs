//! What `${…}` means in a test run (plan 11, milestone 11.11): on top of a
//! scenario's own variables (`@include` parameters, `env:`, `data:`),
//!
//! - `${var:name}`: the backend's vars files;
//! - `${secret:NAME}`: environment variable `NAME`, masked in all output
//!   and artifacts ([`crate::output::mask`]);
//! - `${run.id}` and `${test.id}`: unique per run, and per test try, so
//!   tests against a shared real backend do not collide;
//! - `${saved.NAME.field}`: the JSON answer of an earlier `http … --save
//!   NAME` of the test (or of its file's `@before-all`);
//! - `${backend}`: the backend's name.
//!
//! The plan fills them in to check every step before the run
//! ([`Resolver::checking`]: `${test.id}` and `${saved.…}` get stand-ins,
//! and a `${saved.NAME…}` needs some step of the file to save NAME); the
//! runner fills them in again for each try ([`Resolver::running`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use e2e::scenario::{Scenario, Step, Vars};
use protocol::script::{Resolve, Unresolved, Var};
use serde_json::Value;

/// A run's values.
#[derive(Debug, Clone)]
pub(super) struct RunVars {
    /// The backend's name.
    pub(super) backend: String,
    /// `${var:…}`.
    pub(super) values: BTreeMap<String, String>,
    /// The vars files, for messages.
    pub(super) files: String,
    /// `${run.id}`.
    pub(super) run_id: String,
}

impl RunVars {
    pub(super) fn new(backend: &str, vars: e2e::backend::Vars) -> Self {
        Self {
            backend: backend.to_owned(),
            files: vars.describe(),
            values: vars.values,
            run_id: run_id(),
        }
    }
}

/// `${run.id}`: 8 base-36 characters from the time and the process id.
fn run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut n = (nanos ^ (u128::from(std::process::id()) << 64)) % 36u128.pow(8);
    let mut id = String::new();
    for _ in 0..8 {
        let digit = u32::try_from(n % 36).unwrap_or(0);
        id.push(char::from_digit(digit, 36).unwrap_or('0'));
        n /= 36;
    }
    id
}

/// `${test.id}` of a try: `<run.id>-<test number>`, then `-<try>` for a
/// retry and `-r<run>` for a `--repeat` run after the first.
pub(super) fn test_id(run: &RunVars, index: usize, attempt: u32, repeat: Option<usize>) -> String {
    let mut id = format!("{}-{}", run.run_id, index + 1);
    if let Some(r) = repeat.filter(|&r| r > 1) {
        let _ = write!(id, "-r{r}");
    }
    if attempt > 1 {
        let _ = write!(id, "-{attempt}");
    }
    id
}

/// What `${saved.…}` can be.
enum Saved<'a> {
    /// While checking: the names some step saves.
    Names(&'a BTreeSet<String>),
    /// While running: the answers saved so far.
    Values(&'a BTreeMap<String, Value>),
}

/// Fills in a step's variables.
pub(super) struct Resolver<'a> {
    base: &'a Vars,
    run: &'a RunVars,
    test_id: &'a str,
    saved: Saved<'a>,
}

impl<'a> Resolver<'a> {
    /// For checking a step before the run: `names` are the names the
    /// file's `http --save` steps save.
    pub(super) fn checking(base: &'a Vars, run: &'a RunVars, names: &'a BTreeSet<String>) -> Self {
        Self {
            base,
            run,
            test_id: "0",
            saved: Saved::Names(names),
        }
    }

    /// For running a step.
    pub(super) fn running(
        base: &'a Vars,
        run: &'a RunVars,
        test_id: &'a str,
        saved: &'a BTreeMap<String, Value>,
    ) -> Self {
        Self {
            base,
            run,
            test_id,
            saved: Saved::Values(saved),
        }
    }
}

impl Resolve for Resolver<'_> {
    fn resolve(&self, var: &Var) -> Result<String, Unresolved> {
        match var {
            Var::Name(name) if name == "backend" && !self.base.params.contains_key(name) => {
                Ok(self.run.backend.clone())
            }
            Var::Var(name) => self.run.values.get(name).cloned().ok_or_else(|| {
                Unresolved::new(format!("{var} is not set for backend {}", self.run.backend)).hint(
                    format!(
                        "add `{name} = \"…\"` to {} (vars in [e2e] or [e2e.backends.{}])",
                        self.run.files, self.run.backend
                    ),
                )
            }),
            Var::Secret(name) => secret(var, name),
            Var::Path(path) => match path.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                ["run", "id"] => Ok(self.run.run_id.clone()),
                ["test", "id"] => Ok(match self.test_id {
                    "0" => format!("{}-0", self.run.run_id),
                    id => id.to_owned(),
                }),
                ["saved", name, ref rest @ ..] => self.saved(var, name, rest),
                _ => Err(Unresolved::new(format!("unknown variable {var}")).hint(
                    "the dotted variables are ${run.id}, ${test.id} and ${saved.NAME.field}",
                )),
            },
            _ => self.base.resolve(var),
        }
    }
}

impl Resolver<'_> {
    fn saved(&self, var: &Var, name: &str, path: &[&str]) -> Result<String, Unresolved> {
        match &self.saved {
            Saved::Names(names) if names.contains(name) => Ok("saved".to_owned()),
            Saved::Names(_) => Err(Unresolved::new(format!(
                "{var}: no http step of the file saves {name}"
            ))
            .hint(format!(
                "add --save {name} to the http step whose answer it is, e.g. http POST /api/items --json-body '{{…}}' --save {name}"
            ))),
            Saved::Values(values) => {
                let Some(mut value) = values.get(name) else {
                    return Err(Unresolved::new(format!(
                        "{var}: nothing was saved as {name} yet"
                    ))
                    .hint(format!(
                        "the http … --save {name} step must run before this one (in @before-all, @before or the test)"
                    )));
                };
                for (i, key) in path.iter().enumerate() {
                    let next = match value {
                        Value::Object(map) => map.get(*key),
                        Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
                        _ => None,
                    };
                    let Some(next) = next else {
                        let at = std::iter::once(name)
                            .chain(path[..i].iter().copied())
                            .collect::<Vec<_>>()
                            .join(".");
                        let keys = match value {
                            Value::Object(map) => format!(
                                "it has {}",
                                map.keys().cloned().collect::<Vec<_>>().join(", ")
                            ),
                            Value::Array(items) if items.is_empty() => {
                                "it is an empty list".to_owned()
                            }
                            Value::Array(items) => format!(
                                "it is a list of {} items: use an index from 0 to {}",
                                items.len(),
                                items.len() - 1
                            ),
                            other => format!("it is {other}"),
                        };
                        return Err(Unresolved::new(format!(
                            "{var}: saved.{at} has no {key}"
                        ))
                        .hint(keys));
                    };
                    value = next;
                }
                Ok(match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
            }
        }
    }
}

/// `${secret:NAME}`: environment variable `NAME`, masked from now on.
fn secret(var: &Var, name: &str) -> Result<String, Unresolved> {
    if name.contains('-') {
        return Err(Unresolved::new(format!(
            "{var}: `{name}` cannot be an environment variable name"
        ))
        .hint("name it with letters, digits and _, e.g. ${secret:ADMIN_PASSWORD}"));
    }
    match std::env::var(name) {
        Ok(value) => {
            crate::output::add_secret(name, &value);
            Ok(value)
        }
        Err(_) => Err(Unresolved::new(format!(
            "{var}: environment variable {name} is not set"
        ))
        .hint(format!(
            "export {name}=… before the run; secrets come from the environment and are masked in all output"
        ))),
    }
}

/// The names the steps of `scenario` (its `@include`s too) save with
/// `http … --save NAME`.
pub(crate) fn saved_names(scenario: &Scenario) -> BTreeSet<String> {
    let steps = scenario
        .tests
        .iter()
        .flat_map(|t| &t.steps)
        .chain(&scenario.before_all)
        .chain(&scenario.after_all)
        .chain(&scenario.before)
        .chain(&scenario.after);
    let mut names = BTreeSet::new();
    for step in steps {
        names.extend(save_of(step));
    }
    names
}

/// The NAME of a step `http … --save NAME`.
fn save_of(step: &Step) -> Option<String> {
    let words: Vec<Option<String>> = step
        .line
        .words
        .iter()
        .map(protocol::script::Word::literal)
        .collect();
    if words.first()?.as_deref() != Some("http") {
        return None;
    }
    words.iter().enumerate().find_map(|(i, w)| {
        let w = w.as_deref()?;
        if w == "--save" {
            words.get(i + 1)?.clone()
        } else {
            w.strip_prefix("--save=").map(str::to_owned)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::script::parse;
    use serde_json::json;

    fn run() -> RunVars {
        RunVars {
            backend: "local".into(),
            values: BTreeMap::from([("product".to_owned(), "Keyboard".to_owned())]),
            files: "e2e/vars/local.toml".into(),
            run_id: "abc12345".into(),
        }
    }

    fn expand(line: &str, resolver: &Resolver<'_>) -> Result<String, String> {
        let parsed = parse("t.cdp", line);
        let protocol::script::Item::Command(line) = &parsed.script.items[0] else {
            panic!("not a command");
        };
        line.words
            .iter()
            .map(|w| w.expand(resolver).map_err(|e| e.message))
            .collect::<Result<Vec<_>, _>>()
            .map(|w| w.join(" "))
    }

    #[test]
    fn run_values_fill_in() {
        let base = Vars::default();
        let run = run();
        let saved = BTreeMap::from([(
            "order".to_owned(),
            json!({ "id": 42, "items": [{ "sku": "kb" }], "note": "hi" }),
        )]);
        let r = Resolver::running(&base, &run, "abc12345-3", &saved);
        assert_eq!(
            expand(
                "fill x ${var:product} ${test.id} ${run.id} ${backend} ${saved.order.id} ${saved.order.note}",
                &r
            )
            .unwrap(),
            "fill x Keyboard abc12345-3 abc12345 local 42 hi"
        );
        assert_eq!(
            expand("fill x ${saved.order.items}", &r).unwrap(),
            r#"fill x [{"sku":"kb"}]"#
        );
        assert_eq!(
            expand("fill x ${saved.order.items.0.sku}", &r).unwrap(),
            "fill x kb"
        );
        let e = expand("fill x ${saved.order.items.1.sku}", &r).unwrap_err();
        assert_eq!(e, "${saved.order.items.1.sku}: saved.order.items has no 1");
        let e = expand("fill x ${saved.order.nope}", &r).unwrap_err();
        assert_eq!(e, "${saved.order.nope}: saved.order has no nope");
        let e = expand("fill x ${saved.cart.id}", &r).unwrap_err();
        assert_eq!(e, "${saved.cart.id}: nothing was saved as cart yet");
        let e = expand("fill x ${var:price}", &r).unwrap_err();
        assert_eq!(e, "${var:price} is not set for backend local");
    }

    #[test]
    fn checking_needs_a_step_that_saves() {
        let base = Vars::default();
        let run = run();
        let names = BTreeSet::from(["order".to_owned()]);
        let r = Resolver::checking(&base, &run, &names);
        assert_eq!(
            expand("fill x ${saved.order.id} ${test.id}", &r).unwrap(),
            "fill x saved abc12345-0"
        );
        assert_eq!(
            expand("fill x ${saved.cart.id}", &r).unwrap_err(),
            "${saved.cart.id}: no http step of the file saves cart"
        );
        assert_eq!(
            expand("fill x ${secret:CHROME_CDP_TEST_NO_SUCH_SECRET}", &r).unwrap_err(),
            "${secret:CHROME_CDP_TEST_NO_SUCH_SECRET}: environment variable CHROME_CDP_TEST_NO_SUCH_SECRET is not set"
        );
    }

    #[test]
    fn test_ids_differ_per_try_and_run() {
        let run = run();
        assert_eq!(test_id(&run, 0, 1, None), "abc12345-1");
        assert_eq!(test_id(&run, 2, 2, None), "abc12345-3-2");
        assert_eq!(test_id(&run, 2, 1, Some(1)), "abc12345-3");
        assert_eq!(test_id(&run, 2, 1, Some(4)), "abc12345-3-r4");
        let id = run_id();
        assert_eq!(id.len(), 8);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }
}
