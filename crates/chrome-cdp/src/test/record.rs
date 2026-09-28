//! `test --backend <real> --record-mocks` (plan 11, milestone 11.11): each
//! passed test's fetch and XHR requests, as the real backend answered
//! them, become a mock set of their own: `recorded/<file>/<test>` in the
//! mocks directory (`mock record`'s conversion, plan 11.8), replacing an
//! older one.
//!
//! A set per test, because each test starts from fresh mock state: one
//! request answered differently in two tests (a list before and after
//! another test's write) cannot be one set's answer. A test uses its
//! recording with `@mocks recorded/<file>/<test>`; a person merges what is
//! shared into the project's own sets.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Mutex, PoisonError};

use mock::record::Exchange;
use protocol::schema::MockRecordData;

use super::plan::{Plan, file_rel};

/// Each recorded test's (by index) scope and exchanges.
pub(super) type Recorded = BTreeMap<usize, (Vec<String>, Vec<Exchange>)>;

/// The folder of the recorded sets in the mocks directory.
const FOLDER: &str = "recorded";

/// The set a test's recording is: `recorded/checkout/adds-an-item`.
fn set_name(plan: &Plan, index: usize) -> String {
    let test = &plan.tests[index];
    let file = file_rel(&plan.config, &plan.scenarios[test.file]).with_extension("");
    let slug = test
        .dir
        .file_name()
        .map_or_else(|| "test".into(), |n| n.to_string_lossy().into_owned());
    format!("{FOLDER}/{}/{slug}", file.display())
}

/// Writes the recorded tests' sets; also returns warnings for those that
/// could not be written.
pub(super) fn write(plan: &Plan, record: &Mutex<Recorded>) -> (Vec<MockRecordData>, Vec<String>) {
    let tests = std::mem::take(&mut *record.lock().unwrap_or_else(PoisonError::into_inner));
    let mut written = Vec::new();
    let mut warnings = Vec::new();
    for (index, (scope, exchanges)) in tests {
        let test = &plan.tests[index];
        let name = set_name(plan, index);
        let origin = format!(
            "Recorded by `chrome-cdp test --record-mocks` from backend {}, test \"{}\" ({}:{}).",
            plan.backend.name, test.name, plan.scenarios[test.file].name, test.line
        );
        match crate::mock::record_for_test(&plan.config, &name, &scope, &exchanges, origin) {
            Ok(Some(data)) => written.push(data),
            Ok(None) => {}
            Err(info) => warnings.push(format!("cannot record {name}: {}", info.message)),
        }
    }
    (written, warnings)
}

/// The recorded sets, one line each, for the text output.
pub(super) fn text(recorded: &[MockRecordData]) -> String {
    if recorded.is_empty() {
        return String::new();
    }
    let mut text = String::from("recorded mock sets (a test uses its own with @mocks NAME):\n");
    for data in recorded {
        let _ = writeln!(
            text,
            "  {}: {} {} from {} {} ({})",
            data.set,
            data.mocks,
            if data.mocks == 1 { "mock" } else { "mocks" },
            data.requests,
            if data.requests == 1 {
                "request"
            } else {
                "requests"
            },
            data.path
        );
    }
    text
}
