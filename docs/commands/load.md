<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp load`

Run many users through scenario tests at once, in real browsers, against a real backend

```text
chrome-cdp load [OPTIONS] <PROFILE|PATH>...
```

Arguments:

- `<PROFILE|PATH>`: A load profile: its name in e2e/load (shop for e2e/load/shop.toml) or a .toml file. Or, for a run without one, scenario files, or FILE:LINE for the test at that line: the tests they hold (with --grep and --tags) are the users' journeys

Options:

- `--plan`: Print what the run would do (its stages, the users it is expected to reach, the Chromes it would start), and start nothing
- `--grep <REGEX>`: Only the tests whose `file > name` matches this regular expression (a run without a profile)
- `--tags <TAGS>`: Only tests with one of these tags; `!tag` leaves a tag out, e.g. smoke,!slow (a run without a profile)
- `--backend <NAME>`: The real backend of [e2e.backends.NAME] to load, e.g. staging. Default: the profile's backend, else the config's default_backend (which must be real)
- `--users <N>`: How many users run at once, e.g. 50; each runs the journeys over and over. A run without a profile; default 10
- `--duration <TIME>`: How long users keep starting iterations, e.g. 5m; the ones running then finish. A run without a profile; default 1m
- `--iterations <N>`: Stop each user after this many iterations, e.g. 5 (the run still ends at --duration). A run without a profile
- `--ramp-up <TIME>`: Start the users evenly over this long instead of at once, e.g. 1m. A run without a profile
- `--browsers <N>`: How many Chromes the users share, e.g. 4. Default: enough for --users-per-browser each
- `--users-per-browser <M>`: The most users in one Chrome, each in a browser context of its own, e.g. 10 (default `10`)
- `--think <TIME>`: What a `think` step without a time waits, e.g. 3s, 2s..8s or exp(4s). A run without a profile; default 2s..8s
- `--keep-failures <N>`: Keep the page, console and requests of the first N failed iterations, e.g. 5; later failures are only counted (default `20`)
- `--raw`: Write every step's start, end and result to steps.jsonl in the run's folder
- `--allow-writes`: Run journeys not marked @safe on a protected backend too; `http` writes are then allowed in their steps
- `--browser <BROWSER>`: The browser the users run: headless-shell (chrome-headless-shell, the default when installed; less memory per user) or chrome (full headless Chrome, for apps that need it) (one of `headless-shell`, `chrome`)
- `--headed`: Show the browser windows (in full Chrome)
- `--max-chars <N>`: Bound the JSON output to this many characters (0 = no limit) (default `200000`)

```text
Load testing (docs/e2e/load.md): each virtual user (VU) is a browser
context with one tab in one of the run's Chromes, driven in-process (no
daemon), and runs the selected tests (its journeys) in turn, from a
fresh context each time, until --duration is up (or --iterations are
done); the iterations that are running then finish. Each journey runs as
`test` runs it (${test.id} is unique per iteration, @before and @after run
around it, @before-all and @after-all once), but leanly: a step runs its
action or check and waits for the page to settle as a test step does,
with no action report, mock check or log collection. `think 2s..8s`
steps pause the user (a bare `think` waits --think).

A load run needs a real backend (--backend, or the config's
default_backend); load testing the mocks measures chrome-cdp, not the
site. A protected backend needs --allow-writes, or journeys marked @safe.
Commands for one interactive user (snapshot without a file, screenshot,
debug, refs like e12) are an error before any browser starts.

The run uses --browsers Chromes with up to --users-per-browser users
each (10 by default; past 25, users in one Chrome slow each other
down): chrome-headless-shell when installed (chrome-cdp install
--headless-shell), else full Chrome with a warning; --browser chrome
picks full Chrome. A Chrome that crashes is started again, and its users
record a browser_crash error and go on in the new one. The first
--keep-failures failed iterations keep failure.png, snapshot.txt,
console.txt and requests.txt in
e2e/.results/load/<run>/failures/<n>-vu<V>-i<I>/; later ones are only
counted. --raw writes every step's start, end and result to steps.jsonl
in the run's folder.

The config's [load] section sets what the users' browsers fetch and
keep: block = ["third-party"] (or URL patterns like
"*://*.ads.example/*") fails those requests at once; cache =
"returning" keeps each user's context, cookies and HTTP cache across
its iterations ("new", a fresh context each time, is the default);
renderer_memory = "512MB" makes a page whose heap grows past it a
memory error, and its user goes on in a fresh context.

Examples:
  chrome-cdp load e2e/scenarios/shop.cdp:12 --backend staging --users 50 --duration 5m
  chrome-cdp load e2e/scenarios/shop.cdp --grep browse --users 200 --ramp-up 1m
  chrome-cdp load e2e/load/journeys.cdp --users 20 --iterations 5 --raw
  chrome-cdp load e2e/scenarios/shop.cdp --users 100 --browsers 4 --think 1s..3s
  chrome-cdp load e2e/scenarios/checkout.cdp --backend staging --allow-writes --keep-failures 5
  chrome-cdp load e2e/scenarios/shop.cdp --users 50 --browser chrome
```

