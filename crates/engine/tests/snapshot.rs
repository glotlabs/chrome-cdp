//! `snapshot` against a real Chrome (`CHROME_PATH`; skipped without it
//! unless `CHROME_CDP_REQUIRE_CHROME` is set) and the fixture site
//! (plan 04, milestones 4.1 and 4.3).
//!
//! The corpus (`testdata/site/snapshot/*.html`, from a five-line page to
//! about 50,000 DOM nodes) is read by both engines. Each page's
//! `.expected` file lists lines that must appear, in order and at their
//! depth, and text that must not; the default engine must meet all of it,
//! and the default engine's whole output must match its golden file in
//! `tests/snapshots/` (`UPDATE_GOLDEN=1` rewrites them). The test prints
//! both engines' scores and output tokens: the comparison that chose the
//! default (plan 04's decisions log). Timing is left to
//! `snapshot_latency_benchmark`, which is `#[ignore]`d.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::Host;
use engine::{
    DEFAULT_ENGINE, Engine, NavigateRequest, Navigation, OpenRequest, SnapshotEngine,
    SnapshotRequest, TabCloseRequest, TabNewRequest, WaitUntil, World,
};
use fixtures::Fixtures;
use protocol::schema::{SnapshotData, SnapshotNode};
use protocol::{ErrorInfo, ErrorKind, Output};

const TIMEOUT_MS: u64 = 30_000;

/// The corpus, simple to large.
const CORPUS: &[&str] = &[
    "simple",
    "login",
    "forms",
    "hidden",
    "clickable",
    "lists-tables",
    "widgets",
    "modal",
    "shadow",
    "frames",
    "article",
    "big-app",
    "huge",
];

/// The `-i` snapshot of the big app fixture must stay under this many
/// characters (it is about 7,650 now), so a token regression fails.
const BIG_APP_INTERACTIVE_BUDGET: usize = 8_500;

fn chrome_path() -> Option<PathBuf> {
    match std::env::var_os("CHROME_PATH") {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            assert!(
                std::env::var_os("CHROME_CDP_REQUIRE_CHROME").is_none(),
                "CHROME_CDP_REQUIRE_CHROME is set but CHROME_PATH is not"
            );
            eprintln!("skipping: set CHROME_PATH to run tests against a real Chrome");
            None
        }
    }
}

async fn start(path: &Path) -> Arc<Engine> {
    let engine = Engine::new("engine-snapshot-test", None, None);
    let chrome = browser::discover(Some(path), &Host::current()).unwrap();
    engine
        .open(OpenRequest {
            chrome: Some(chrome),
            timeout_ms: TIMEOUT_MS,
            ..OpenRequest::default()
        })
        .await
        .unwrap();
    engine
}

fn request() -> SnapshotRequest {
    SnapshotRequest {
        tab: None,
        interactive: false,
        scope: None,
        depth: None,
        diff: false,
        all_refs: false,
        all: false,
        text_limit: 200,
        no_text: false,
        engine: None,
        strict_refs: false,
        testid_attribute: None,
        timeout_ms: TIMEOUT_MS,
        max_chars: 0,
    }
}

async fn snapshot(engine: &Engine, req: SnapshotRequest) -> Output {
    engine
        .snapshot(req)
        .await
        .unwrap_or_else(|e| panic!("snapshot: {}", e.to_text()))
}

async fn goto(engine: &Engine, url: &str) {
    engine
        .navigate(NavigateRequest {
            navigation: Navigation::Goto {
                url: url.to_owned(),
                base_url: None,
            },
            wait: WaitUntil::Settle,
            tab: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("goto {url}: {}", e.to_text()));
}

/// Opens `url` in a new tab (so refs start at e1) and makes it active.
async fn new_tab(engine: &Engine, url: &str) {
    engine
        .tab_new(TabNewRequest {
            url: Some(url.to_owned()),
            base_url: None,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap_or_else(|e| panic!("tab new {url}: {}", e.to_text()));
}

async fn close_tab(engine: &Engine) {
    engine
        .tab_close(TabCloseRequest {
            tab: None,
            close_browser: false,
            timeout_ms: TIMEOUT_MS,
        })
        .await
        .unwrap();
}

async fn eval(engine: &Engine, js: &str) -> serde_json::Value {
    engine
        .evaluate(None, None, World::Main, js, Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("{js}: {}", e.to_text()))
}

/// The fixture origins as placeholders, so outputs do not depend on ports.
fn normalize(text: &str, site: &Fixtures) -> String {
    text.replace(site.a(), "{{ORIGIN_A}}")
        .replace(site.b(), "{{ORIGIN_B}}")
}

/// Output lines with every ref as `[ref]`, the header left out.
fn comparable(text: &str) -> Vec<String> {
    let refs = regex::Regex::new(r"\[e\d+\]").unwrap();
    let scopes = regex::Regex::new(r"--scope e\d+").unwrap();
    text.lines()
        .skip(1)
        .map(|l| {
            let l = refs.replace_all(l, "[ref]");
            scopes.replace_all(&l, "--scope [ref]").into_owned()
        })
        .collect()
}

/// How an output meets an `.expected` file.
#[derive(Debug, Default, Clone, Copy)]
struct Score {
    /// Expected lines found, in order.
    matched: usize,
    expected: usize,
    /// Lines with text that must not be there.
    forbidden: usize,
}

impl Score {
    fn perfect(self) -> bool {
        self.matched == self.expected && self.forbidden == 0
    }
}

/// Scores `lines` against an `.expected` file: the longest common
/// subsequence with the expected lines, and the lines holding a `!` text.
fn score(expected: &str, lines: &[String]) -> (Score, Vec<String>) {
    let mut want = Vec::new();
    let mut banned = Vec::new();
    for line in expected.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        match line.strip_prefix('!') {
            Some(text) => banned.push(text.to_owned()),
            None => want.push(line.to_owned()),
        }
    }
    // LCS table.
    let (n, m) = (want.len(), lines.len());
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if want[i] == lines[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut missing = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n {
        if j < m && want[i] == lines[j] {
            i += 1;
            j += 1;
        } else if j < m && table[i][j + 1] >= table[i + 1][j] {
            j += 1;
        } else {
            missing.push(format!("missing: {}", want[i]));
            i += 1;
        }
    }
    let found: Vec<String> = lines
        .iter()
        .filter(|l| banned.iter().any(|b| l.contains(b.as_str())))
        .map(|l| format!("forbidden: {l}"))
        .collect();
    let score = Score {
        matched: table[0][0],
        expected: n,
        forbidden: found.len(),
    };
    missing.extend(found);
    (score, missing)
}

/// Output tokens, counted the way BPE tokenizers split text: a run of
/// letters, up to three digits, or one other non-space character each.
/// Deterministic and close enough to compare engines and formats.
fn tokens(text: &str) -> usize {
    let re = regex::Regex::new(r"[A-Za-z]+|[0-9]{1,3}|[^\sA-Za-z0-9]").unwrap();
    re.find_iter(text).count()
}

fn corpus_dir() -> PathBuf {
    fixtures::site_dir().join("snapshot")
}

fn golden_path(page: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{page}.txt"))
}

/// Compares `actual` with the golden file of `page`, or rewrites it with
/// `UPDATE_GOLDEN=1`. Returns a problem, if any.
fn check_golden(page: &str, actual: &str) -> Option<String> {
    let path = golden_path(page);
    let actual = format!("{actual}\n");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(expected) if expected == actual => None,
        Ok(expected) => Some(format!(
            "{} differs (UPDATE_GOLDEN=1 accepts it):\n--- expected\n{expected}--- actual\n{actual}",
            path.display()
        )),
        Err(e) => Some(format!(
            "{}: {e} (UPDATE_GOLDEN=1 writes it)",
            path.display()
        )),
    }
}

/// ARIA roles an agent may copy from a snapshot into a `role=` locator:
/// the controls (not chrome-cdp's own `clickable` and `iframe`, nor
/// Chrome's `colorwell`) and the named parts of a page.
const ARIA_ROLES: &[&str] = &[
    "heading",
    "row",
    "cell",
    "columnheader",
    "rowheader",
    "region",
    "navigation",
    "form",
    "dialog",
    "alertdialog",
    "tablist",
    "tabpanel",
    "menu",
    "tree",
    "button",
    "link",
    "textbox",
    "searchbox",
    "checkbox",
    "radio",
    "switch",
    "combobox",
    "listbox",
    "option",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "slider",
    "spinbutton",
    "treeitem",
];

/// `role=<role>[name="<name>"]` for every named node of those roles the
/// default snapshot shows (in an iframe: `frame=<its ref> >> …`), up to
/// `limit`.
fn role_locators(nodes: &[SnapshotNode], frame: Option<&str>, limit: usize, out: &mut Vec<String>) {
    for node in nodes {
        if out.len() >= limit {
            return;
        }
        if ARIA_ROLES.contains(&node.role.as_str())
            && !node.name.is_empty()
            && !node.name.contains(['"', '\\'])
        {
            let locator = format!("role={}[name=\"{}\"]", node.role, node.name);
            out.push(match frame {
                Some(frame) => format!("frame={frame} >> {locator}"),
                None => locator,
            });
        }
        let inner = if node.role == "iframe" {
            node.reference.as_deref()
        } else {
            frame
        };
        role_locators(&node.children, inner, limit, out);
    }
}

/// An agent copies a role and name from the snapshot into a `role=`
/// locator: it must find the element. The snapshot's roles and names are
/// Chrome's; `role=` computes its own in the page (engine B's code).
#[tokio::test(flavor = "multi_thread")]
async fn role_locators_find_what_the_snapshot_shows() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let options = engine::LocateOptions::default();
    let mut problems = Vec::new();
    let mut checked = 0;
    for page in CORPUS {
        new_tab(&engine, &site.url(&format!("/snapshot/{page}.html"))).await;
        let output = snapshot(
            &engine,
            SnapshotRequest {
                text_limit: 0,
                ..request()
            },
        )
        .await;
        let data: SnapshotData = serde_json::from_value(output.data).unwrap();
        let mut locators = Vec::new();
        role_locators(&data.nodes, None, 25, &mut locators);
        for locator in locators {
            checked += 1;
            let found = engine
                .call_on_all(
                    None,
                    &locator,
                    &options,
                    "function () { return this.length; }",
                    Duration::from_millis(TIMEOUT_MS),
                )
                .await;
            match found {
                Ok(call) if call.value.as_u64().unwrap_or(0) > 0 => {}
                // The isolated world cannot see into closed shadow roots.
                Ok(_) if locator.contains("Closed shadow") => {}
                Ok(_) => problems.push(format!("{page}: {locator} finds nothing")),
                Err(e) => problems.push(format!("{page}: {locator}: {}", e.to_text())),
            }
        }
        close_tab(&engine).await;
    }
    assert!(checked > 100, "only {checked} locators checked");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn corpus_meets_expected_trees() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let other = match DEFAULT_ENGINE {
        SnapshotEngine::Ax => SnapshotEngine::Dom,
        SnapshotEngine::Dom => SnapshotEngine::Ax,
    };
    let mut problems = Vec::new();
    let mut table = vec![format!(
        "{:<13} {:>16} {:>16} {:>8} {:>8}",
        "page", "ax lines", "dom lines", "ax tok", "dom tok"
    )];
    let mut totals = [(0usize, 0usize, 0usize); 2];
    for page in CORPUS {
        let expected =
            std::fs::read_to_string(corpus_dir().join(format!("{page}.expected"))).unwrap();
        new_tab(&engine, &site.url(&format!("/snapshot/{page}.html"))).await;
        let mut row = Vec::new();
        // The default engine first, so its refs are the golden ones.
        for kind in [DEFAULT_ENGINE, other] {
            let output = snapshot(
                &engine,
                SnapshotRequest {
                    engine: Some(kind),
                    ..request()
                },
            )
            .await;
            let text = normalize(&output.text, site);
            let (score, misses) = score(&expected, &comparable(&text));
            let tokens = tokens(&text);
            if kind == DEFAULT_ENGINE {
                if let Some(problem) = check_golden(page, &text) {
                    problems.push(problem);
                }
                if !score.perfect() {
                    problems.push(format!(
                        "{page} ({}): {}",
                        kind.as_str(),
                        misses.join("\n  ")
                    ));
                }
            } else if !misses.is_empty() {
                eprintln!("{page} ({}):\n  {}", kind.as_str(), misses.join("\n  "));
            }
            row.push((kind, score, tokens));
        }
        row.sort_by_key(|(kind, _, _)| kind.as_str());
        let cell = |s: Score| {
            format!(
                "{}/{}{}",
                s.matched,
                s.expected,
                if s.forbidden > 0 {
                    format!(" !{}", s.forbidden)
                } else {
                    String::new()
                }
            )
        };
        for (i, (_, s, t)) in row.iter().enumerate() {
            totals[i].0 += s.matched;
            totals[i].1 += s.expected;
            totals[i].2 += t;
        }
        table.push(format!(
            "{page:<13} {:>16} {:>16} {:>8} {:>8}",
            cell(row[0].1),
            cell(row[1].1),
            row[0].2,
            row[1].2
        ));
        close_tab(&engine).await;
    }
    table.push(format!(
        "{:<13} {:>16} {:>16} {:>8} {:>8}",
        "total",
        format!("{}/{}", totals[0].0, totals[0].1),
        format!("{}/{}", totals[1].0, totals[1].1),
        totals[0].2,
        totals[1].2
    ));
    eprintln!("{}", table.join("\n"));
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn views_scope_depth_and_limits() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/snapshot/login.html")).await;

    // Refs are the same across views and snapshots.
    let full = snapshot(&engine, request()).await;
    let again = snapshot(&engine, request()).await;
    assert_eq!(full.text, again.text, "deterministic");
    let interactive = snapshot(
        &engine,
        SnapshotRequest {
            interactive: true,
            ..request()
        },
    )
    .await;
    assert!(
        interactive
            .text
            .contains("- form \"Login\"\n    - textbox \"Email\" [e4] required")
    );
    assert!(
        !interactive.text.contains("heading"),
        "{}",
        interactive.text
    );
    for line in interactive.text.lines().skip(1) {
        let content = line.trim_start().trim_start_matches("- ");
        assert!(
            full.text
                .lines()
                .any(|l| l.trim_start().trim_start_matches("- ") == content),
            "{line} not in the full snapshot"
        );
    }

    // --scope by ref, and by CSS.
    let form_ref = |text: &str| {
        text.lines()
            .find(|l| l.contains("textbox \"Email\""))
            .and_then(|l| l.split('[').nth(1))
            .map(|r| {
                r.trim_end_matches(|c| c != ']')
                    .trim_end_matches(']')
                    .to_owned()
            })
            .unwrap()
    };
    let email = form_ref(&full.text);
    let scoped = snapshot(
        &engine,
        SnapshotRequest {
            scope: Some(email.clone()),
            ..request()
        },
    )
    .await;
    assert_eq!(
        scoped.text.lines().nth(1),
        Some(format!("- textbox \"Email\" [{email}] required").as_str())
    );
    let scoped = snapshot(
        &engine,
        SnapshotRequest {
            scope: Some("css=form".into()),
            ..request()
        },
    )
    .await;
    assert_eq!(scoped.text.lines().nth(1), Some("- form \"Login\""));
    assert_eq!(scoped.text.lines().count(), 7, "{}", scoped.text);
    let err = engine
        .snapshot(SnapshotRequest {
            scope: Some("#nothing-here".into()),
            ..request()
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    let err = engine
        .snapshot(SnapshotRequest {
            scope: Some("e999".into()),
            ..request()
        })
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    assert_eq!(err.message, "tab t1 has no ref e999");

    // --depth counts what it leaves out.
    let shallow = snapshot(
        &engine,
        SnapshotRequest {
            depth: Some(1),
            ..request()
        },
    )
    .await;
    assert!(
        shallow
            .text
            .contains("- main\n  - … 8 more below (use a larger --depth)"),
        "{}",
        shallow.text
    );

    // --max-chars cuts at a line and says how to see the rest.
    let cut = snapshot(
        &engine,
        SnapshotRequest {
            max_chars: 300,
            ..request()
        },
    )
    .await;
    assert!(cut.text.chars().count() <= 300);
    assert!(
        cut.text
            .lines()
            .last()
            .unwrap()
            .starts_with("… snapshot truncated: ")
    );

    // --json holds the same tree.
    let data: SnapshotData = serde_json::from_value(full.data.clone()).unwrap();
    assert_eq!(data.title, "Sign in – Acme");
    assert_eq!(data.engine, DEFAULT_ENGINE.as_str());
    let main = data.nodes.iter().find(|n| n.role == "main").unwrap();
    let form = main.children.iter().find(|n| n.role == "form").unwrap();
    assert_eq!(form.name, "Login");
    let email = &form.children[0];
    assert_eq!(email.role, "textbox");
    assert_eq!(email.states, ["required"]);
    assert!(email.reference.is_some());
    let password = &form.children[1];
    assert!(password.password && password.value.is_none());

    // --no-text and --all-refs.
    let skeleton = snapshot(
        &engine,
        SnapshotRequest {
            no_text: true,
            ..request()
        },
    )
    .await;
    assert!(!skeleton.text.contains("- text "), "{}", skeleton.text);
    let all = snapshot(
        &engine,
        SnapshotRequest {
            all_refs: true,
            ..request()
        },
    )
    .await;
    assert!(
        all.text.contains("- heading \"Sign in\" [e"),
        "{}",
        all.text
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_shows_what_changed() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/snapshot/login.html")).await;
    let diff = |(): ()| SnapshotRequest {
        diff: true,
        ..request()
    };
    let first = snapshot(&engine, diff(())).await;
    assert!(
        first
            .text
            .contains("(no earlier snapshot of this tab; all of it is new)"),
        "{}",
        first.text
    );
    let unchanged = snapshot(&engine, diff(())).await;
    assert_eq!(
        unchanged.text.lines().nth(1),
        Some("(no changes since the last snapshot)")
    );
    eval(
        &engine,
        "document.querySelector('input[type=checkbox]').checked = true;
         document.querySelector('[role=alert]').remove();
         const b = document.createElement('button'); b.textContent = 'Try again';
         document.querySelector('form').appendChild(b); true",
    )
    .await;
    let changed = snapshot(&engine, diff(())).await;
    let body: Vec<&str> = changed.text.lines().skip(1).collect();
    assert_eq!(
        body,
        [
            "  - main",
            "    - form \"Login\"",
            "~     - checkbox \"Remember me\" [e6] checked (was unchecked)",
            "+     - button \"Try again\" [e12]",
            "-     - alert \"Invalid password\"",
        ],
        "{}",
        changed.text
    );
    let data: SnapshotData = serde_json::from_value(changed.data.clone()).unwrap();
    assert_eq!(data.diff, Some(true));
    let form = &data.nodes[0].children[0];
    assert_eq!(form.change, None);
    assert_eq!(form.children[0].change.as_deref(), Some("changed"));
    assert_eq!(form.children[0].was.as_deref(), Some("unchecked"));
    assert!(!data.new_page);

    // After a reload the nodes are new: the diff says it matched by place.
    goto(&engine, &site.url("/snapshot/login.html")).await;
    let reloaded = snapshot(&engine, diff(())).await;
    let mut lines = reloaded.text.lines().skip(1);
    assert_eq!(
        lines.next(),
        Some("(a new page since the last snapshot; its nodes are compared by place)"),
        "{}",
        reloaded.text
    );
    assert!(
        reloaded.text.contains("+     - alert \"Invalid password\""),
        "{}",
        reloaded.text
    );
    let data: SnapshotData = serde_json::from_value(reloaded.data.clone()).unwrap();
    assert!(data.new_page);
    // Only the first diff after the load says so.
    let again = snapshot(&engine, diff(())).await;
    assert_eq!(
        again.text.lines().nth(1),
        Some("(no changes since the last snapshot)")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dom_engine_reads_the_same_page() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/snapshot/login.html")).await;
    let ax = snapshot(
        &engine,
        SnapshotRequest {
            engine: Some(SnapshotEngine::Ax),
            ..request()
        },
    )
    .await;
    let dom = snapshot(
        &engine,
        SnapshotRequest {
            engine: Some(SnapshotEngine::Dom),
            ..request()
        },
    )
    .await;
    // Same page, same nodes, same refs.
    assert_eq!(ax.text, dom.text);
    let scoped = snapshot(
        &engine,
        SnapshotRequest {
            engine: Some(SnapshotEngine::Dom),
            scope: Some("css=form".into()),
            ..request()
        },
    )
    .await;
    assert_eq!(scoped.text.lines().nth(1), Some("- form \"Login\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn interactive_big_app_stays_within_its_budget() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/snapshot/big-app.html")).await;
    let output = snapshot(
        &engine,
        SnapshotRequest {
            interactive: true,
            ..request()
        },
    )
    .await;
    let chars = output.text.chars().count();
    eprintln!("big-app -i: {chars} chars, {} tokens", tokens(&output.text));
    assert!(
        chars <= BIG_APP_INTERACTIVE_BUDGET,
        "{chars} chars:\n{}",
        output.text
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_page_fails_at_once() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    goto(&engine, &site.url("/snapshot/simple.html")).await;
    eval(&engine, "setTimeout(() => confirm('Sure?'), 0); true").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let err: ErrorInfo = engine.snapshot(request()).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::DialogOpen, "{}", err.to_text());
}

/// Snapshot latency per engine and page (plan 04's gate: p95 < 150 ms at
/// 5k nodes, < 1 s at 50k). Benchmarks run after all plans are in:
/// `cargo test -p chrome-cdp-engine --test snapshot -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a benchmark; run on an idle machine"]
async fn snapshot_latency_benchmark() {
    let Some(path) = chrome_path() else { return };
    let site = Fixtures::shared();
    let engine = start(&path).await;
    let runs: usize = std::env::var("CHROME_CDP_BENCH_RUNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    for page in CORPUS {
        goto(&engine, &site.url(&format!("/snapshot/{page}.html"))).await;
        let nodes = eval(&engine, "document.getElementsByTagName('*').length").await;
        for kind in [SnapshotEngine::Ax, SnapshotEngine::Dom] {
            let mut times = Vec::with_capacity(runs);
            for _ in 0..runs {
                let start = Instant::now();
                snapshot(
                    &engine,
                    SnapshotRequest {
                        engine: Some(kind),
                        max_chars: 20_000,
                        ..request()
                    },
                )
                .await;
                times.push(start.elapsed());
            }
            times.sort();
            // The nearest-rank percentile.
            let at = |percent: usize| times[(times.len() * percent).div_ceil(100).max(1) - 1];
            eprintln!(
                "{page:<13} {:>6} elements  {:<3}  p50 {:>8.1?}  p95 {:>8.1?}",
                nodes,
                kind.as_str(),
                at(50),
                at(95)
            );
        }
    }
}

#[test]
fn scoring_finds_missing_and_forbidden_lines() {
    let expected = "# comment\n- main\n  - button \"Go\" [ref]\n!Secret\n";
    let lines = |s: &str| s.lines().map(str::to_owned).collect::<Vec<_>>();
    let (good, misses) = score(
        expected,
        &lines("- main\n  - text \"x\"\n  - button \"Go\" [ref]"),
    );
    assert!(good.perfect(), "{misses:?}");
    let (bad, misses) = score(
        expected,
        &lines("- main\n- button \"Go\" [ref]\n- text \"Secret\""),
    );
    assert_eq!((bad.matched, bad.expected, bad.forbidden), (1, 2, 1));
    assert_eq!(
        misses,
        [
            "missing:   - button \"Go\" [ref]",
            "forbidden: - text \"Secret\""
        ]
    );
    assert_eq!(tokens("- link \"Pricing\" [e12] → /pricing"), 12);
    assert_eq!(
        comparable("page t1 \"x\" u\n- list [e4]\n  - … 3 more listitem (use --scope e4 --all)"),
        [
            "- list [ref]",
            "  - … 3 more listitem (use --scope [ref] --all)"
        ]
    );
}
