//! `explore`'s text form, and the scenario `-o` writes for each state.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use protocol::format;
use protocol::schema::{ExploreData, ExploreElement, ExploreSkip, ExploreState, ExploreStep};

use super::rules::origin;

/// How many problems of each kind a state's lines list.
const LISTED: usize = 5;

/// `link "Products"`; an element without a role by its tag.
pub(super) fn element_text(element: &ExploreElement) -> String {
    let what = if element.role.is_empty() || element.role == "generic" {
        element.tag.as_str()
    } else {
        element.role.as_str()
    };
    if element.name.is_empty() {
        what.to_owned()
    } else {
        format!("{what} {:?}", element.name)
    }
}

/// `click link "Products"`, `click link "Team" (new tab)`, `goto /x`.
pub(super) fn step_text(step: &ExploreStep, start: &str) -> String {
    match (&step.element, step.new_tab) {
        (Some(element), true) => format!("click {} (new tab)", element_text(element)),
        (Some(element), false) => format!("{} {}", step.action, element_text(element)),
        (None, _) => format!(
            "{} {}",
            step.action,
            shown(step.url.as_deref().unwrap_or_default(), start)
        ),
    }
}

/// A URL on the start's origin as its path (`/products?page=2`), others
/// whole.
pub(super) fn shown(url: &str, start: &str) -> String {
    match (origin(url), origin(start)) {
        (Some(a), Some(b)) if a == b && a != "file://" => {
            let rest = &url[a.len()..];
            if rest.is_empty() {
                "/".to_owned()
            } else {
                rest.to_owned()
            }
        }
        _ => url.to_owned(),
    }
}

/// `ok`, or what went wrong: `1 error, 2 failed requests, 3 layout`.
fn verdict(state: &ExploreState) -> String {
    let mut parts = Vec::new();
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    parts.extend(count(state.errors.len(), "error", "errors"));
    parts.extend(count(
        state.failed_requests.len(),
        "failed request",
        "failed requests",
    ));
    if let Some(layout) = &state.layout {
        parts.extend(count(layout.len(), "layout", "layout"));
    }
    if parts.is_empty() {
        "ok".to_owned()
    } else {
        parts.join(", ")
    }
}

/// What ended the walk, as the header says it.
fn stopped(data: &ExploreData) -> String {
    let budget = &data.budget;
    match data.stopped_by.as_str() {
        "done" => "all visited".to_owned(),
        "depth" => format!("all visited to --depth {}", budget.depth),
        "max-states" => format!("--max-states {}", budget.max_states),
        "max-actions" => format!("--max-actions {}", budget.max_actions),
        "time" => format!(
            "--time {}",
            format::duration(Duration::from_millis(budget.time_ms))
        ),
        other => other.to_owned(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `explore`'s text form: a header, a line per state (with its problems
/// under it), then what was broken, not followed, left and failed.
pub(super) fn explore_text(
    data: &ExploreData,
    lines: &dyn Fn(&protocol::schema::Event) -> String,
) -> String {
    let start = &data.start;
    let mut text = format!(
        "explore {start}: {}, {}, {} (stopped: {})",
        plural(data.states.len(), "state", "states"),
        plural(
            usize::try_from(data.actions).unwrap_or(usize::MAX),
            "action",
            "actions"
        ),
        format::duration(Duration::from_millis(data.duration_ms)),
        stopped(data)
    );
    let rows: Vec<Vec<String>> = data
        .states
        .iter()
        .map(|s| {
            let mut title = format!("{:?}", s.title);
            for dialog in &s.dialogs {
                if dialog.is_empty() {
                    title.push_str(" + dialog");
                } else {
                    let _ = write!(title, " + dialog {dialog:?}");
                }
            }
            let from = match (s.path.last(), data.edges.iter().find(|e| e.to == s.id)) {
                (Some(step), Some(edge)) => format!("← {} {}", edge.from, step_text(step, start)),
                _ => String::new(),
            };
            vec![s.id.clone(), shown(&s.url, start), title, verdict(s), from]
        })
        .collect();
    let table = format::table(&rows);
    for (line, state) in table.lines().zip(&data.states) {
        let _ = write!(text, "\n{line}");
        let mut problems: Vec<String> = Vec::new();
        for list in [&state.errors, &state.failed_requests] {
            problems.extend(list.iter().take(LISTED).map(lines));
            if list.len() > LISTED {
                problems.push(format!("… and {} more", list.len() - LISTED));
            }
        }
        if let Some(layout) = &state.layout {
            problems.extend(
                layout
                    .iter()
                    .take(LISTED)
                    .map(|f| format!("layout {}: {}", f.kind, f.message)),
            );
            if layout.len() > LISTED {
                problems.push(format!(
                    "… and {} more layout findings",
                    layout.len() - LISTED
                ));
            }
        }
        for problem in problems {
            let _ = write!(text, "\n      ! {problem}");
        }
    }
    for broken in &data.broken {
        let what = match (broken.status, &broken.error) {
            (_, Some(error)) => error.clone(),
            (Some(status), None) => status.to_string(),
            (None, None) => "no page".to_owned(),
        };
        let _ = write!(
            text,
            "\nbroken: {} {what} (from {} {})",
            shown(&broken.url, start),
            broken.from,
            step_text(&broken.step, start).trim_start_matches("click ")
        );
    }
    let not_followed = not_followed(data);
    if !not_followed.is_empty() {
        let _ = write!(text, "\nnot followed: {}", not_followed.join(", "));
    }
    if !data.unvisited.is_empty() {
        let mut by_state: BTreeMap<usize, (&str, usize)> = BTreeMap::new();
        for left in &data.unvisited {
            let n = left.from[1..].parse().unwrap_or(usize::MAX);
            by_state.entry(n).or_insert((&left.from, 0)).1 += 1;
        }
        let each: Vec<String> = by_state
            .values()
            .map(|(id, n)| format!("{id} {n}"))
            .collect();
        let _ = write!(
            text,
            "\nleft unvisited: {} ({})",
            plural(data.unvisited.len(), "step", "steps"),
            each.join(", ")
        );
    }
    for failed in &data.failed {
        match &failed.step {
            Some(step) => {
                let _ = write!(
                    text,
                    "\nfailed: {} {}: {}",
                    failed.from,
                    step_text(step, start),
                    failed.error
                );
            }
            None => {
                let _ = write!(text, "\nfailed: {}: {}", failed.from, failed.error);
            }
        }
    }
    if let Some(dir) = &data.dir {
        let _ = write!(text, "\nwrote {dir}");
    }
    text
}

/// The `not followed:` parts, one per kind.
fn not_followed(data: &ExploreData) -> Vec<String> {
    let skipped = &data.skipped;
    let mut parts = Vec::new();
    if !skipped.external.is_empty() {
        let mut hosts: Vec<String> = Vec::new();
        for skip in &skipped.external {
            let host = skip
                .url
                .as_deref()
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| {
                    u.host_str().map(|h| match u.port() {
                        Some(port) => format!("{h}:{port}"),
                        None => h.to_owned(),
                    })
                })
                .unwrap_or_default();
            if !host.is_empty() && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        let n = skipped.external.len();
        let what = plural(n, "external link", "external links");
        parts.push(if hosts.is_empty() {
            what
        } else {
            format!("{what} ({})", hosts.join(", "))
        });
    }
    let named = |list: &[ExploreSkip]| -> String {
        let shown: Vec<String> = list
            .iter()
            .take(3)
            .map(|s| format!("{} on {}", element_text(&s.element), s.from))
            .collect();
        let more = if list.len() > 3 { ", …" } else { "" };
        format!("{}{more}", shown.join(", "))
    };
    let states = |list: &[ExploreSkip]| -> String {
        let mut ids: Vec<&str> = Vec::new();
        for skip in list {
            if !ids.contains(&skip.from.as_str()) {
                ids.push(&skip.from);
            }
        }
        ids.join(", ")
    };
    if !skipped.destructive.is_empty() {
        parts.push(format!(
            "{} ({})",
            plural(skipped.destructive.len(), "destructive", "destructive"),
            named(&skipped.destructive)
        ));
    }
    if !skipped.forms.is_empty() {
        parts.push(format!(
            "{} ({}; --submit-forms)",
            plural(skipped.forms.len(), "form submit", "form submits"),
            states(&skipped.forms)
        ));
    }
    if !skipped.files.is_empty() {
        parts.push(format!(
            "{} ({})",
            plural(skipped.files.len(), "file", "files"),
            states(&skipped.files)
        ));
    }
    if !skipped.other.is_empty() {
        parts.push(format!(
            "{} ({})",
            plural(
                skipped.other.len(),
                "mailto/tel/javascript link",
                "mailto/tel/javascript links"
            ),
            states(&skipped.other)
        ));
    }
    parts
}

/// `states/<id>.cdp`: load the start, then each step of the path, so
/// `chrome-cdp run` gets to the state.
pub(super) fn scenario(state: &ExploreState, start: &str) -> String {
    let mut text = format!(
        "# explore {}: {} {:?}\ngoto {}\n",
        state.id,
        shown(&state.url, start),
        state.title,
        format::shell_quote(start)
    );
    for step in &state.path {
        match (step.action.as_str(), &step.locator, &step.url) {
            ("goto", _, Some(url)) => {
                let _ = writeln!(text, "goto {}", format::shell_quote(url));
            }
            (_, Some(locator), _) => {
                let _ = writeln!(text, "click {}", format::shell_quote(locator));
            }
            _ => {}
        }
    }
    text
}
