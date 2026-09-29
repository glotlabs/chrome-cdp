//! Soak tests, ignored because they run for minutes; the nightly job runs
//! them with `cargo xtask soak`.
//!
//! - Plan 10.1: 1,000 commands drawn at random from a weighted mix, against
//!   the fixture site. No crash (the daemon keeps its pid and every command
//!   ends with an exit code the contract allows), bounded RSS, and a p99
//!   latency within budget. `CHROME_CDP_SOAK_COMMANDS` and
//!   `CHROME_CDP_SOAK_SEED` change the count and the draw.
//! - Plan 06's criterion on bounded buffers: the daemon's RSS stays under
//!   150 MB after a 1-hour soak on a page that logs 100 messages a second.
//!   `CHROME_CDP_SOAK_SECS` shortens the hour.
//!
//! Run them with
//! `CHROME_PATH=… cargo test -p chrome-cdp --release --test soak -- --ignored --nocapture`.

#![allow(clippy::unwrap_used, clippy::print_stderr)]

mod common;

use std::process::Command;
use std::time::{Duration, Instant};

use common::{Sandbox, chrome_path, code, json_out, stderr, stdout};
use fixtures::Fixtures;

const RSS_LIMIT_KB: u64 = 150 * 1024;
const SAMPLE_EVERY: Duration = Duration::from_secs(10);

/// The resident set size of `pid` in KB (`ps` reports it the same way on
/// Linux and macOS).
fn rss_kb(pid: u32) -> u64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[test]
#[ignore = "benchmark; run after all plans are implemented"]
fn daemon_memory_stays_bounded_under_constant_logging() {
    let Some(chrome) = chrome_path() else { return };
    let soak = std::env::var("CHROME_CDP_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(Duration::from_secs(3600), Duration::from_secs);
    let sandbox = Sandbox::with_chrome(&chrome);
    let page = Fixtures::shared().url("/console/soak.html");
    let out = sandbox.run(&["open", &page]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let status = json_out(&sandbox.run(&["status", "--json"]));
    let pid = u32::try_from(status["data"]["daemon_pid"].as_u64().unwrap()).unwrap();

    let start = Instant::now();
    let mut peak = 0;
    while start.elapsed() < soak {
        std::thread::sleep(SAMPLE_EVERY);
        // Read now and then, as an agent would; the buffers must stay
        // bounded whether or not anyone reads them.
        for args in [["console", "--limit", "5"], ["network", "--limit", "5"]] {
            let out = sandbox.run(&args);
            assert_eq!(code(&out), 0, "{}", stderr(&out));
        }
        let rss = rss_kb(pid);
        peak = peak.max(rss);
        eprintln!("{:>5}s: daemon RSS {} KB", start.elapsed().as_secs(), rss);
    }

    // The page kept logging the whole time and the log holds only the
    // newest entries.
    let out = sandbox.run(&["console", "--since", "0", "--limit", "5000", "--json"]);
    let entries = json_out(&out)["data"]["entries"].as_array().unwrap().len();
    assert!(
        entries <= 1000,
        "the console buffer holds {entries} entries"
    );
    let rss = rss_kb(pid);
    eprintln!("daemon RSS at the end {rss} KB, peak {peak} KB");
    assert!(rss < RSS_LIMIT_KB, "the daemon's RSS is {rss} KB");
}

/// The daemon's RSS limit in the command soak: the same as the logging
/// soak's, as neither may grow with the number of commands.
const MIX_RSS_LIMIT_KB: u64 = 150 * 1024;
/// The p99 budget of one command, page work included (a `goto` loads a
/// page; most commands take a few milliseconds).
const MIX_P99: Duration = Duration::from_secs(2);
/// Every command's `--timeout`, so a stuck one shows up as a timeout
/// instead of stalling the soak.
const MIX_TIMEOUT: &str = "5s";

/// The pages the soak visits: static and framework pages without dialogs,
/// popups or downloads, so any click on them keeps the session usable.
const MIX_PAGES: &[&str] = &[
    "/apps/react.html",
    "/apps/vue.html",
    "/apps/svelte.html",
    "/nav/a.html",
    "/nav/b.html",
    "/nav/spa.html",
    "/snapshot/login.html",
    "/snapshot/forms.html",
    "/snapshot/lists-tables.html",
    "/snapshot/widgets.html",
    "/snapshot/article.html",
    "/frames/nested.html",
    "/actions/frameworks.html",
];

/// xorshift64*: a seeded draw without a dependency, the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        (!items.is_empty()).then(|| &items[self.below(items.len())])
    }
}

/// What the soak knows of the page: the refs of the last `snapshot -i`,
/// by role, and how many tabs are open.
#[derive(Default)]
struct Seen {
    refs: Vec<(String, String)>,
    tabs: usize,
}

impl Seen {
    fn refs_of(&self, roles: &[&str]) -> Vec<String> {
        self.refs
            .iter()
            .filter(|(role, _)| roles.contains(&role.as_str()))
            .map(|(_, r)| r.clone())
            .collect()
    }
}

/// Reads `- role "name" [e12] …` lines of a snapshot.
fn snapshot_refs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start().strip_prefix("- ")?;
            let role = line.split_whitespace().next()?;
            let start = line.find("[e")? + 1;
            let end = start + line[start..].find(']')?;
            Some((role.to_owned(), line[start..end].to_owned()))
        })
        .collect()
}

/// The weighted mix: (weight, kind). Reading commands dominate, as in an
/// agent's session; every kind in `command_for` has a weight here.
const MIX: &[(u32, &str)] = &[
    (10, "goto"),
    (14, "snapshot"),
    (12, "click"),
    (7, "fill"),
    (3, "check"),
    (5, "text"),
    (5, "eval"),
    (3, "screenshot"),
    (4, "console"),
    (4, "network"),
    (3, "back"),
    (2, "forward"),
    (3, "reload"),
    (2, "tab-new"),
    (2, "tab-close"),
    (2, "tabs"),
    (3, "press"),
    (3, "scroll"),
    (3, "find"),
    (3, "expect"),
    (2, "wait"),
    (2, "status"),
    (2, "cookies"),
];

fn draw_kind(rng: &mut Rng) -> &'static str {
    let total: u32 = MIX.iter().map(|(w, _)| w).sum();
    let mut at = u32::try_from(rng.next() % u64::from(total)).unwrap();
    for (weight, kind) in MIX {
        if at < *weight {
            return kind;
        }
        at -= weight;
    }
    unreachable!()
}

/// The arguments of one command of `kind`, from what the soak has seen.
/// `None` when the kind cannot run now (no ref to click, one tab to close).
fn command_for(kind: &str, rng: &mut Rng, seen: &Seen, shot: &str) -> Option<Vec<String>> {
    let site = Fixtures::shared();
    let page = site.url(MIX_PAGES[rng.below(MIX_PAGES.len())]);
    let args: Vec<String> = match kind {
        "goto" => vec!["goto".into(), page],
        "snapshot" => vec!["snapshot".into(), "-i".into()],
        "click" => {
            let r = rng
                .pick(&seen.refs_of(&["button", "link", "checkbox", "radio", "tab"]))?
                .clone();
            vec!["click".into(), r]
        }
        "fill" => {
            let r = rng.pick(&seen.refs_of(&["textbox", "searchbox"]))?.clone();
            vec!["fill".into(), r, format!("soak {}", rng.below(1000))]
        }
        "check" => {
            let r = rng.pick(&seen.refs_of(&["checkbox"]))?.clone();
            vec!["check".into(), r]
        }
        "text" => vec!["text".into(), "--max-chars".into(), "2000".into()],
        "eval" => vec![
            "eval".into(),
            "document.title + ' ' + location.pathname".into(),
        ],
        "screenshot" => vec!["screenshot".into(), "-o".into(), shot.into()],
        "console" => vec!["console".into(), "--limit".into(), "5".into()],
        "network" => vec!["network".into(), "--limit".into(), "5".into()],
        "back" => vec!["back".into()],
        "forward" => vec!["forward".into()],
        "reload" => vec!["reload".into()],
        "tab-new" if seen.tabs < 4 => vec!["tab".into(), "new".into(), page],
        "tab-close" if seen.tabs > 1 => vec!["tab".into(), "close".into()],
        "tabs" => vec!["tabs".into()],
        "press" => vec![
            "press".into(),
            (*rng.pick(&["Tab", "Shift+Tab", "Escape", "ArrowDown"])?).into(),
        ],
        "scroll" => vec![
            "scroll".into(),
            "--by".into(),
            format!("0,{}", rng.below(600)),
        ],
        "find" => vec![
            "find".into(),
            (*rng.pick(&["button", "link", "name", "item"])?).into(),
        ],
        "expect" => vec!["expect".into(), "css=body".into(), "visible".into()],
        "wait" => vec!["wait".into(), "--load".into()],
        "status" => vec!["status".into()],
        "cookies" => vec!["cookies".into()],
        _ => return None,
    };
    Some(args)
}

/// The value at quantile `q` (0..=1) of sorted durations.
fn quantile(sorted: &[Duration], q: f64) -> Duration {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let at = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[at]
}

#[test]
#[ignore = "soak test; the nightly job runs it (cargo xtask soak)"]
fn random_command_mix_does_not_crash_or_grow() {
    let Some(chrome) = chrome_path() else { return };
    let count: usize = std::env::var("CHROME_CDP_SOAK_COMMANDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let seed: u64 = std::env::var("CHROME_CDP_SOAK_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5eed);
    eprintln!("soak: {count} commands, seed {seed} (CHROME_CDP_SOAK_SEED)");
    let sandbox = Sandbox::with_chrome(&chrome);
    let session = sandbox.session();
    let shot = sandbox.dir.path().join("shot.png");
    let shot = shot.to_str().unwrap();
    session.ok(&["open", &Fixtures::shared().url(MIX_PAGES[0])]);
    let pid = session.json(&["status"])["daemon_pid"].as_u64().unwrap();
    let pid = u32::try_from(pid).unwrap();

    let mut rng = Rng(seed | 1);
    let mut seen = Seen {
        tabs: 1,
        ..Seen::default()
    };
    let mut latencies: Vec<Duration> = Vec::with_capacity(count);
    let mut by_exit = std::collections::BTreeMap::<i32, usize>::new();
    let mut peak = 0;
    let mut ran = 0;
    while ran < count {
        let kind = draw_kind(&mut rng);
        let Some(mut args) = command_for(kind, &mut rng, &seen, shot) else {
            continue;
        };
        ran += 1;
        args.extend(["--timeout".into(), MIX_TIMEOUT.into()]);
        let start = Instant::now();
        let out = session.cli().args(&args).output().unwrap();
        latencies.push(start.elapsed());
        let exit = out.status.code().unwrap_or_else(|| {
            panic!(
                "`chrome-cdp {}` died from a signal: {}",
                args.join(" "),
                stderr(&out)
            )
        });
        *by_exit.entry(exit).or_default() += 1;
        // 0 ok, 1 page-level (a stale ref, nothing to go back to, …),
        // 3 a timeout, 5 a dialog. Never 2 (the mix's own bug) or 4 (the
        // daemon or browser is gone).
        assert!(
            matches!(exit, 0 | 1 | 3 | 5),
            "command {ran} `chrome-cdp {}` exited {exit}: {}",
            args.join(" "),
            stderr(&out)
        );
        if exit == 5 {
            session.ok(&["dialog", "dismiss"]);
        }
        match (kind, exit) {
            ("snapshot", 0) => seen.refs = snapshot_refs(&stdout(&out)),
            ("goto" | "back" | "forward" | "reload" | "click" | "tab-new" | "tab-close", _) => {
                seen.refs.clear();
            }
            _ => {}
        }
        if matches!(kind, "tab-new" | "tab-close" | "click") {
            seen.tabs = session.json(&["tabs"])["tabs"].as_array().unwrap().len();
        }
        if ran % 100 == 0 {
            let rss = rss_kb(pid);
            peak = peak.max(rss);
            eprintln!("{ran:>5} commands: daemon RSS {rss} KB, exits {by_exit:?}");
        }
    }

    // No crash: the same daemon still answers.
    let now = session.json(&["status"])["daemon_pid"].as_u64().unwrap();
    assert_eq!(now, u64::from(pid), "the daemon restarted during the soak");
    let timeouts = by_exit.get(&3).copied().unwrap_or(0);
    assert!(
        timeouts * 50 <= count,
        "{timeouts} of {count} commands timed out"
    );
    let rss = rss_kb(pid);
    peak = peak.max(rss);
    latencies.sort();
    let (p50, p99) = (quantile(&latencies, 0.5), quantile(&latencies, 0.99));
    eprintln!(
        "soak done: exits {by_exit:?}; daemon RSS {rss} KB (peak {peak} KB); \
         latency p50 {p50:?}, p99 {p99:?}, max {:?}",
        latencies.last().unwrap()
    );
    assert!(
        peak < MIX_RSS_LIMIT_KB,
        "the daemon's RSS peaked at {peak} KB"
    );
    assert!(p99 <= MIX_P99, "p99 latency {p99:?} is over {MIX_P99:?}");
}

#[test]
fn snapshot_refs_reads_roles_and_refs() {
    let text = "page t1 \"Sign in\" http://x/\n- form \"Login\"\n  - textbox \"Email\" [e1] required\n  - button \"Sign in\" [e4]\n";
    assert_eq!(
        snapshot_refs(text),
        [
            ("textbox".into(), "e1".into()),
            ("button".into(), "e4".into())
        ]
    );
}

#[test]
fn every_mix_kind_makes_a_command() {
    let mut rng = Rng(1);
    let seen = Seen {
        refs: vec![
            ("button".into(), "e1".into()),
            ("textbox".into(), "e2".into()),
            ("checkbox".into(), "e3".into()),
        ],
        tabs: 2,
    };
    for (_, kind) in MIX {
        assert!(
            command_for(kind, &mut rng, &seen, "/tmp/x.png").is_some(),
            "{kind}"
        );
    }
}
