//! UI coverage (plan 18, milestone 18.4): `test --ui-coverage` records
//! which pages the tests reached, the controls those pages showed, and
//! which of them a step acted on or checked; and the mocks of the tests'
//! sets, with whether a request matched each.
//!
//! The runner reads `snapshot -i` (the elements with refs) after a step
//! that changed the URL and at the end of each test, and `snapshot -i
//! --diff` after the other actions, whose added nodes are the controls a
//! dialog or menu brought; at most one per step. A control's key is its
//! page, role and name, plus its container (the nearest landmark, form,
//! region or dialog) when another control of the page has that role and
//! name. The controls a step touched come from the step's fingerprint
//! (plan 18.2), keyed the same way when the report is made.
//!
//! Each test's [`Coverage`] is merged into the run's in the plan's order,
//! so `--jobs` does not change the result.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use protocol::schema::{
    Fingerprint, MockRef, MockSetsData, RoutedRequest, UiCoverageControl, UiCoverageData,
    UiCoverageMock, UiCoveragePage, UiCoverageRequest,
};
use serde_json::Value;

/// The file it writes, in the artifacts folder.
pub(super) const FILE: &str = "ui-coverage.json";

/// How many characters of a name count: a fingerprint keeps 80.
const NAME_CHARS: usize = 80;

/// How many of a container's name count: a fingerprint keeps 40.
const CONTAINER_CHARS: usize = 40;

/// How many pages the summary lists.
const PAGES_SHOWN: usize = 20;

/// How many controls a page's line names that no step acted on.
const NEVER_SHOWN: usize = 3;

/// How many unused mocks, or requests, the summary names.
const MOCKS_SHOWN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Container {
    role: String,
    name: String,
}

impl Container {
    fn new(role: &str, name: &str) -> Option<Self> {
        (engine::is_landmark(role) && role != "iframe").then(|| Self {
            role: role.to_owned(),
            name: cut(&normalize(name), CONTAINER_CHARS),
        })
    }

    fn text(&self) -> String {
        named(&self.role, &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Control {
    role: String,
    name: String,
    container: Option<Container>,
}

impl Control {
    fn same_element(&self, other: &Self) -> bool {
        self.role == other.role && self.name == other.name
    }
}

#[derive(Debug, Clone, Default)]
struct Page {
    path: String,
    /// The tests that visited it, by `file:line`.
    tests: BTreeSet<String>,
    controls: Vec<Control>,
}

/// A control a step touched.
#[derive(Debug, Clone)]
struct Touch {
    page: String,
    test: String,
    control: Control,
    /// Acted on (`click`, `fill`, …), not only checked.
    acted: bool,
}

/// What one test (or a run) reached and touched.
#[derive(Debug, Clone, Default)]
pub(super) struct Coverage {
    pages: Vec<Page>,
    touched: Vec<Touch>,
    mocks: Vec<UiCoverageMock>,
    requests: Vec<UiCoverageRequest>,
}

/// Where a snapshot is taken from: what the runner asks for after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Snap {
    /// `snapshot -i`: every control of the page.
    Full,
    /// `snapshot -i --diff`: the controls added since the tab's last one.
    Added,
}

impl Snap {
    /// The command, less `--json`.
    pub(super) fn words(self) -> &'static [&'static str] {
        match self {
            Self::Full => &[
                "snapshot",
                "-i",
                "--all",
                "--text-limit",
                "0",
                "--max-chars",
                "0",
            ],
            Self::Added => &[
                "snapshot",
                "-i",
                "--diff",
                "--all",
                "--text-limit",
                "0",
                "--max-chars",
                "0",
            ],
        }
    }
}

impl Coverage {
    /// Records the controls of `snapshot`'s data (`SnapshotData`) as
    /// test `test` saw them; with [`Snap::Added`], only the added ones,
    /// unless the diff had nothing to compare with or a new page.
    pub(super) fn snapshot(&mut self, data: &Value, snap: Snap, test: &str, patterns: &[String]) {
        let Some(path) = data["url"].as_str().and_then(path_of) else {
            return;
        };
        let all = snap == Snap::Full || data["diff"] == false || data["new_page"] == true;
        let page = page_of(path, patterns);
        let at = self.page(&page);
        self.pages[at].tests.insert(test.to_owned());
        let mut found = Vec::new();
        if let Some(nodes) = data["nodes"].as_array() {
            walk(nodes, None, all, &mut found);
        }
        let controls = &mut self.pages[at].controls;
        for control in found {
            if !controls.contains(&control) {
                controls.push(control);
            }
        }
    }

    /// Records that a step of test `test` touched the element of
    /// fingerprint `fp`: acted on it, or only checked it.
    pub(super) fn touch(&mut self, fp: &Fingerprint, acted: bool, test: &str, patterns: &[String]) {
        if fp.role.is_empty() || fp.page.is_empty() {
            return;
        }
        self.touched.push(Touch {
            page: page_of(&fp.page, patterns),
            test: test.to_owned(),
            control: Control {
                role: fp.role.clone(),
                name: cut(&normalize(&fp.name), NAME_CHARS),
                container: fp
                    .container
                    .as_ref()
                    .and_then(|c| Container::new(&c.role, &c.name)),
            },
            acted,
        });
    }

    /// Records the mocks of a test's sets (`mock list`'s data) and how
    /// many requests each answered, with those the mock server answered
    /// (`served`, its journal of the test).
    pub(super) fn mocks(&mut self, list: &Value, served: &[RoutedRequest]) {
        let Ok(list) = serde_json::from_value::<MockSetsData>(list.clone()) else {
            return;
        };
        // Listed in the order they are tried: the last applied first.
        for set in list.sets.iter().rev() {
            for info in &set.mocks {
                let hits = info.hits
                    + served
                        .iter()
                        .filter(|r| r.mock.as_ref().is_some_and(|m| same_mock(m, &info.mock)))
                        .count() as u64;
                self.add_mock(info.mock.clone(), hits);
            }
        }
        for entry in served {
            if let Some(mock) = &entry.mock
                && !self.mocks.iter().any(|m| same_mock(&m.mock, mock))
            {
                let hits = served
                    .iter()
                    .filter(|r| r.mock.as_ref().is_some_and(|m| same_mock(m, mock)))
                    .count() as u64;
                self.add_mock(mock.clone(), hits);
            }
        }
    }

    /// Records the API requests (fetch and XHR) of a test against a real
    /// backend: `(method, url, resource type)`.
    pub(super) fn requests<'a>(
        &mut self,
        requests: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
    ) {
        for (method, url, kind) in requests {
            if !matches!(kind, "fetch" | "xhr") {
                continue;
            }
            let Some(path) = path_of(url) else { continue };
            self.add_request(UiCoverageRequest {
                method: method.to_owned(),
                path: engine::fingerprint_score::page_template(path),
                count: 1,
            });
        }
    }

    /// Adds `other`'s (a later test's, or another run's).
    pub(super) fn merge(&mut self, other: Self) {
        for page in other.pages {
            let at = self.page(&page.path);
            let mine = &mut self.pages[at];
            mine.tests.extend(page.tests);
            for control in page.controls {
                if !mine.controls.contains(&control) {
                    mine.controls.push(control);
                }
            }
        }
        self.touched.extend(other.touched);
        for mock in other.mocks {
            self.add_mock(mock.mock, mock.hits);
        }
        for request in other.requests {
            self.add_request(request);
        }
    }

    /// The report: each touched control found among its page's controls,
    /// and the keys; `patterns` are `[e2e] pages`.
    pub(super) fn report(&self, patterns: &[String]) -> UiCoverageData {
        let mut pages = self.pages.clone();
        // Acted on and checked, by page and control.
        let mut counts: Vec<Vec<(u64, u64)>> = pages
            .iter()
            .map(|p| vec![(0, 0); p.controls.len()])
            .collect();
        for touch in &self.touched {
            let at = if let Some(at) = pages.iter().position(|p| p.path == touch.page) {
                at
            } else {
                pages.push(Page {
                    path: touch.page.clone(),
                    ..Page::default()
                });
                counts.push(Vec::new());
                pages.len() - 1
            };
            let page = &mut pages[at];
            page.tests.insert(touch.test.clone());
            let Some(control) = find(&mut page.controls, &touch.control) else {
                continue;
            };
            counts[at].resize(page.controls.len(), (0, 0));
            let count = &mut counts[at][control];
            if touch.acted {
                count.0 += 1;
            } else {
                count.1 += 1;
            }
        }
        let mut data = UiCoverageData {
            file: None,
            controls: 0,
            acted: 0,
            checked: 0,
            pages: Vec::new(),
            unvisited: patterns
                .iter()
                .filter(|p| !pages.iter().any(|page| &page.path == *p))
                .cloned()
                .collect(),
            mocks: self.mocks.clone(),
            requests: {
                let mut requests = self.requests.clone();
                requests.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
                requests
            },
        };
        for (page, counts) in pages.iter().zip(&counts) {
            let controls: Vec<UiCoverageControl> = page
                .controls
                .iter()
                .enumerate()
                .map(|(i, control)| {
                    let shared = page
                        .controls
                        .iter()
                        .filter(|c| c.same_element(control))
                        .count()
                        > 1;
                    let container = control
                        .container
                        .as_ref()
                        .filter(|_| shared)
                        .map(Container::text);
                    let mut key = named(&control.role, &control.name);
                    if let Some(container) = &container {
                        let _ = write!(key, " in {container}");
                    }
                    let (acted, checked) = counts.get(i).copied().unwrap_or_default();
                    UiCoverageControl {
                        key,
                        role: control.role.clone(),
                        name: control.name.clone(),
                        container,
                        acted,
                        checked,
                    }
                })
                .collect();
            let acted = controls.iter().filter(|c| c.acted > 0).count();
            let checked = controls
                .iter()
                .filter(|c| c.acted == 0 && c.checked > 0)
                .count();
            data.controls += controls.len();
            data.acted += acted;
            data.checked += checked;
            data.pages.push(UiCoveragePage {
                page: page.path.clone(),
                tests: page.tests.len(),
                acted,
                checked,
                controls,
            });
        }
        data
    }

    /// The index of page `page`, added if new.
    fn page(&mut self, page: &str) -> usize {
        if let Some(at) = self.pages.iter().position(|p| p.path == page) {
            return at;
        }
        self.pages.push(Page {
            path: page.to_owned(),
            ..Page::default()
        });
        self.pages.len() - 1
    }

    fn add_mock(&mut self, mock: MockRef, hits: u64) {
        match self.mocks.iter_mut().find(|m| same_mock(&m.mock, &mock)) {
            Some(known) => known.hits += hits,
            None => self.mocks.push(UiCoverageMock { mock, hits }),
        }
    }

    fn add_request(&mut self, request: UiCoverageRequest) {
        let known = self
            .requests
            .iter_mut()
            .find(|r| r.method == request.method && r.path == request.path);
        match known {
            Some(known) => known.count += request.count,
            None => self.requests.push(request),
        }
    }
}

/// The control of `controls` that `touched` is: the one with its role and
/// name (and container, when several have them); one it adds when none
/// has and it is a control, else `None` (a text an `expect` checked).
fn find(controls: &mut Vec<Control>, touched: &Control) -> Option<usize> {
    let alike: Vec<usize> = controls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.same_element(touched))
        .map(|(i, _)| i)
        .collect();
    match alike.as_slice() {
        [] if engine::is_interactive(&touched.role) && touched.role != "iframe" => {
            controls.push(touched.clone());
            Some(controls.len() - 1)
        }
        [] => None,
        [one] => Some(*one),
        [first, ..] => Some(
            alike
                .iter()
                .copied()
                .find(|&i| controls[i].container == touched.container)
                .unwrap_or(*first),
        ),
    }
}

/// The controls (nodes with refs, but frames) of `nodes`; with `all`
/// false, only those marked added. `container` is the one around them.
fn walk(nodes: &[Value], container: Option<&Container>, all: bool, found: &mut Vec<Control>) {
    for node in nodes {
        let change = node["change"].as_str();
        if change == Some("removed") {
            continue;
        }
        let role = node["role"].as_str().unwrap_or_default();
        let name = node["name"].as_str().unwrap_or_default();
        if node.get("ref").is_some_and(Value::is_string)
            && role != "iframe"
            && (all || change == Some("added"))
        {
            found.push(Control {
                role: role.to_owned(),
                name: cut(&normalize(name), NAME_CHARS),
                container: container.cloned(),
            });
        }
        let inner = Container::new(role, name);
        if let Some(children) = node["children"].as_array() {
            walk(children, inner.as_ref().or(container), all, found);
        }
    }
}

/// The path of `url` (an `http(s)` or `file` URL), without its query and
/// fragment; `None` for `about:blank` and the like.
fn path_of(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https" | "file") {
        return None;
    }
    let path = &rest[rest.find('/').unwrap_or(rest.len())..];
    let path = path.split(['?', '#']).next().unwrap_or_default();
    Some(if path.is_empty() { "/" } else { path })
}

/// The page `path` is: the first of `patterns` (`[e2e] pages`) it
/// matches, else its template (`/products/:id`).
pub(super) fn page_of(path: &str, patterns: &[String]) -> String {
    let path = match path.strip_suffix('/') {
        Some(p) if !p.is_empty() => p,
        _ => path,
    };
    patterns
        .iter()
        .find(|p| matches(p, path))
        .cloned()
        .unwrap_or_else(|| engine::fingerprint_score::page_template(path))
}

/// Whether `path` matches page pattern `pattern`: segment by segment, a
/// `:name` or `*` segment matches any one, and a last `**` the rest.
fn matches(pattern: &str, path: &str) -> bool {
    let pattern = match pattern.strip_suffix('/') {
        Some(p) if !p.is_empty() => p,
        _ => pattern,
    };
    let mut want = pattern.split('/');
    let mut have = path.split('/');
    loop {
        match (want.next(), have.next()) {
            (None, None) => return true,
            (Some("**"), _) => return want.next().is_none(),
            (Some(w), Some(h)) => {
                let any = (w == "*" || w.starts_with(':')) && !h.is_empty();
                if !any && w != h {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

fn same_mock(a: &MockRef, b: &MockRef) -> bool {
    a.file == b.file && a.line == b.line
}

/// `role "name"`, or `role` without a name.
fn named(role: &str, name: &str) -> String {
    if name.is_empty() {
        role.to_owned()
    } else {
        format!("{role} \"{name}\"")
    }
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// The summary printed after the run.
pub(super) fn text(data: &UiCoverageData, listed: bool) -> String {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut text = format!(
        "ui coverage: {}, {}, {} acted on ({}%)",
        plural(data.pages.len(), "page", "pages"),
        plural(data.controls, "control", "controls"),
        data.acted,
        percent(data.acted, data.controls)
    );
    if data.checked > 0 {
        let _ = write!(text, ", {} more only checked", data.checked);
    }
    text.push('\n');
    // On how many pages a control with a role and name is.
    let shown_on = |role: &str, name: &str| {
        data.pages
            .iter()
            .filter(|p| p.controls.iter().any(|c| c.role == role && c.name == name))
            .count()
    };
    let width = data
        .pages
        .iter()
        .take(PAGES_SHOWN)
        .map(|p| p.page.chars().count())
        .max()
        .unwrap_or(0)
        .min(30);
    for page in data.pages.iter().take(PAGES_SHOWN) {
        let _ = write!(
            text,
            "{:width$} {} of {} acted on",
            page.page,
            page.acted,
            page.controls.len()
        );
        if page.checked > 0 {
            let _ = write!(text, ", {} only checked", page.checked);
        }
        // A page's own controls first: the header every page shows would
        // fill every line.
        let mut never: Vec<&UiCoverageControl> =
            page.controls.iter().filter(|c| c.acted == 0).collect();
        never.sort_by_key(|c| shown_on(&c.role, &c.name));
        let never: Vec<&str> = never.iter().map(|c| c.key.as_str()).collect();
        if !never.is_empty() {
            let _ = write!(
                text,
                "; never: {}",
                never[..never.len().min(NEVER_SHOWN)].join(", ")
            );
            if never.len() > NEVER_SHOWN {
                text.push_str(", …");
            }
        }
        text.push('\n');
    }
    if data.pages.len() > PAGES_SHOWN {
        let more = data.pages.len() - PAGES_SHOWN;
        let _ = writeln!(
            text,
            "… {more} more {} in {FILE}",
            if more == 1 { "page" } else { "pages" }
        );
    }
    if !data.unvisited.is_empty() {
        let _ = writeln!(text, "never visited: {}", data.unvisited.join(", "));
    } else if !listed {
        text.push_str(
            "pages no test visited are not known: list them in [e2e] pages, or find them with `chrome-cdp explore`\n",
        );
    }
    let unused: Vec<String> = data
        .mocks
        .iter()
        .filter(|m| m.hits == 0)
        .map(|m| format!("{} \"{}\"", m.mock.at, m.mock.name))
        .collect();
    if !unused.is_empty() {
        let _ = write!(
            text,
            "mocks never used: {}",
            unused[..unused.len().min(MOCKS_SHOWN)].join(", ")
        );
        if unused.len() > MOCKS_SHOWN {
            let _ = write!(text, ", … {} more", unused.len() - MOCKS_SHOWN);
        }
        text.push('\n');
    } else if !data.mocks.is_empty() {
        let _ = writeln!(text, "mocks: all {} used", data.mocks.len());
    }
    if !data.requests.is_empty() {
        let shown: Vec<String> = data
            .requests
            .iter()
            .take(MOCKS_SHOWN)
            .map(|r| format!("{} {} ×{}", r.method, r.path, r.count))
            .collect();
        let _ = write!(text, "api requests: {}", shown.join(", "));
        if data.requests.len() > MOCKS_SHOWN {
            let _ = write!(text, ", … {} more", data.requests.len() - MOCKS_SHOWN);
        }
        text.push('\n');
    }
    if let Some(file) = &data.file {
        let _ = writeln!(text, "{FILE}: {file}");
    }
    text
}

fn percent(part: usize, whole: usize) -> usize {
    (part * 100 + whole / 2).checked_div(whole).unwrap_or(0)
}

#[cfg(test)]
mod tests;
