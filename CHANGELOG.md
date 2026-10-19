# Changelog

All notable changes to chrome-cdp are recorded here, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

chrome-cdp follows [Semantic Versioning](https://semver.org/). Its public
API is the command line (commands, arguments, flags, environment
variables, exit codes and error kinds) and the `--json` output, whose
schemas `chrome-cdp schema <command>` prints: from 1.0, a minor release
may only add to them, and anything else needs a major release. The plain
text output is for reading and may change in any release.

To release: move the Unreleased entries under a `## [X.Y.Z] - date`
heading, set that version in `Cargo.toml` (`[workspace.package]` and the
internal dependencies), then push the tag `vX.Y.Z`. The release workflow
checks both with `cargo xtask release check --tag vX.Y.Z` (which also
fails on a breaking change to the schemas or the command line since the
previous tag, unless the major version went up), and puts the section at
the top of the GitHub release notes.

## [Unreleased]

### Added

- The first release: every command of plans 01-11 (see `chrome-cdp help`).
- Release archives for Linux and macOS (x86_64 and arm64) with SHA-256
  sums, installable with `install.sh`, Homebrew, `cargo install`,
  `cargo binstall` or npm (`npx chrome-cdp`).
- `chrome-cdp upgrade` updates a standalone install from the GitHub
  releases, checking the archive's checksum.
- `chrome-cdp doctor` checks Chrome, its libraries and sandbox, the
  session directory, macOS quarantine and version skew, and prints fixes.
- Every frame, every worker (plan 13): network, routes, mocks, console,
  errors, emulation, determinism and coverage apply to out-of-process
  iframes and dedicated, shared and service workers, including ones that
  attach later. `frames` prints a tab's iframes and workers.
- Locators, timers and more (plan 14): `has=`, `has-not=`, `has-not-text=`,
  `and=` and `or=` in locators; `locator <loc>` prints a ref's stable
  locator; paused timers (`time pause`, `run-for`, `pause-at`);
  `keyboard down|up`; HTML and image clipboard; directory uploads;
  `extensions`; `open --init-script`; read commands that wait for their
  element; HAR pages per navigation; `pdf` header and footer templates;
  `expose` functions that return a value; WebSocket frames in `network`;
  `expect-snapshot` templates with wildcards and regexes.
- From page to source (plan 15): `source <loc>` (the file, line and
  component chain behind an element, from React, Vue and Svelte debug
  info or the stack that made the node), `components [loc]` (props and state), logpoints
  (`debug log FILE:LINE EXPR`), `errors --locals`, request initiators in
  `network show`, and `wait --hmr` with hot updates in action reports.
- Seeing more (plan 16): `why <loc>` (why an action would wait),
  `layout` and `expect --layout` (overlap, clipping, overflow, small
  targets, contrast), `snapshot --budget` and `--expand`, `forms` and
  `fill-form`, `compare` (two pages, one diff), and `explore` (map an
  app).
- Page audits (plan 17): `issues` (Chrome's Issues panel), `audit` and
  `expect --audit` with a baseline, `expect --perf` budgets, and more
  accessibility rules.
- Tests that maintain themselves (plan 18): `test --heal` (writes a fix
  for a broken locator), flake diagnosis with `--repeat`,
  `test --ui-coverage`, `repro start|save` bundles that `test` replays
  offline, and a recorder (`record`).
- Mocks (plan 19): multipart bodies, nested collections,
  `--match-bodies` for `mock record`, `mock import` and
  `test --record-mocks`; WebSocket and SSE recording, `mock sse send`,
  and `mock serve` answering WebSocket upgrades; `mock serve --upstream`
  for a hybrid backend; `mock gen --openapi` and the spec check in
  `test` (`[e2e] mock_check`).
- Load testing with real browsers (plan 23.1): `chrome-cdp load` runs
  many users at once through the scenario tests against a real backend,
  each in a browser context of one of several Chromes driven in-process;
  `think` pauses a load run's user (a no-op elsewhere); failed iterations
  keep their page, console and requests, and `--raw` writes every
  step's timing. `testdata/load/server` is a load server with latency
  and errors per endpoint, and `cargo xtask bench load` measures users
  per machine.
- A lean browser for load runs (plan 23.2): `load` runs its users in
  chrome-headless-shell when installed (`--browser chrome` for full
  Chrome), with flags of its own; the config's `[load]` section can
  `block` third-party or matching requests in Chrome itself, keep each
  user's cache and cookies across iterations (`cache = "returning"`), and
  stop a page whose heap grows past `renderer_memory` (a `memory` error).
  `load --json` reports the browser, the cache and the contexts made.

### Changed

- A mocked CORS preflight names no mock when the mock matches on headers
  or a body, which a preflight does not show.
- `console` and `network` with `--context` and no `--tab` read the
  context's active tab, not the browser's.

### Removed

- `chrome-cdp convert`, which translated test files from another tool
  into `.cdp` scenarios.
- The benchmark comparisons with other browser tools: `micro`, `tokens`
  and `agent` now measure chrome-cdp alone.
