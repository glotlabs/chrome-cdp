# chrome-cdp

**A browser for AI agents, driven from the shell.** chrome-cdp is a single
static binary that drives Chrome over the Chrome DevTools Protocol. An
agent runs one command per step: `snapshot` shows the page as a compact
accessibility tree with short refs, `click e6` acts on one, and every
action reports what it caused (a navigation, a console error, a failed
request) so the agent rarely needs to look again. Errors say what happened
and the next command to run. A background daemon keeps the browser warm
between commands, and there is nothing else to install: no Node, no
driver, no patched browser.

It is built to replace Playwright for agents: to open, inspect, operate
and verify web pages, debug a local dev server, and write and run E2E
tests, with a fraction of the tokens.

## Install

```sh
curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh | sh
brew install glotlabs/tap/chrome-cdp
npx chrome-cdp --help                 # or npm install -g chrome-cdp
cargo binstall chrome-cdp             # or cargo install chrome-cdp
docker run --rm ghcr.io/glotlabs/chrome-cdp doctor
```

Linux and macOS, x86_64 and arm64. chrome-cdp uses the Chrome or Chromium
you have, or `chrome-cdp install` downloads Chrome for Testing.
`chrome-cdp doctor` checks the setup. [docs/install.md](docs/install.md)
has the details.

## Quickstart

```sh
chrome-cdp open localhost:3000              # start a browser and load the app
chrome-cdp snapshot -i                      # what can be acted on: - button "Sign in" [e6]
chrome-cdp fill label=Email ada@example.com # filled e3 textbox "Email"
chrome-cdp click e6                         # clicked e6 button "Sign in"
                                            # → navigated to …/dashboard "Dashboard" (200)
chrome-cdp expect 'role=heading' text Welcome   # passed: 'role=heading' text Welcome
chrome-cdp errors                           # the page's exceptions, with source-mapped stacks
```

For an agent, `chrome-cdp skill --install` adds the chrome-cdp skill to
the project's `.claude/skills/`, which teaches Claude Code (or any Agent
Skills client) the workflow in about 1,100 tokens. `chrome-cdp --help`
fits on one screen.

## E2E tests with a mocked backend

`chrome-cdp test` runs a web frontend's end-to-end tests from committed
scenario files and mock files, against a mocked backend (fast,
deterministic, offline) or, with the same scenarios, against a real one:

```
# e2e/scenarios/cart.cdp
@mocks base

@test "adds an item to the cart"
  goto /products/42
  click 'role=button[name="Add to cart"]'
  expect 'role=status' text "1 item"
  expect request POST /api/cart --json-body '{"product":42}'
```

`chrome-cdp init e2e` sets up a project in one command, and `chrome-cdp
history --as-scenario` turns what you (or an agent) just did in the
browser into a test. **[docs/e2e/](docs/e2e/README.md)** has the quickstart,
scenarios, mock files with state and fake REST collections, recipes, and
running the suite against a real backend.

## Compared with Playwright

| | chrome-cdp | Playwright MCP | Playwright CLI |
|---|---|---|---|
| What you install | one static binary (≈20 MB) | Node.js, an npm package, a browser download | Node.js, an npm package, a browser download |
| How an agent calls it | a shell command per step | MCP tool calls | a shell command per step |
| Page snapshot tokens, 13-page corpus ([benchmarks](docs/benchmarks.md#token-suite)) | **14,625** (`snapshot`), **11,593** (`snapshot -i`) | 476,225 | not measured |
| … on a 50,000-node page | **7,752** | 415,654 | not measured |
| What an action returns | a short report: navigation, dialogs, errors, failed requests | the page's snapshot | — |
| Errors | a kind, an exit code, and the next command to run | Playwright's error message | — |
| E2E tests | built in: `.cdp` scenarios, mock files, `chrome-cdp test` | no | no (`@playwright/test` is separate) |
| Raw CDP | `chrome-cdp cdp Domain.method` | no | — |
| Browsers | Chrome and Chromium | Chromium, Firefox, WebKit | Chromium, Firefox, WebKit |
| Agent task success, tokens and time ([agent benchmark](docs/benchmarks.md#agent-task-benchmark)) | pending | pending | not measured |
| Warm command latency ([micro benchmark](docs/benchmarks.md#micro-suite)) | pending | pending | not measured |

Token counts use the offline `o200k_base` tokenizer; the benchmarks page
has the method and the per-page numbers. A dash is a cell nobody has
checked yet. The agent and latency benchmarks
are built but have not been run yet. Coming from Playwright?
[docs/migrating-from-playwright.md](docs/migrating-from-playwright.md)
maps its API to commands, [docs/playwright-parity.md](docs/playwright-parity.md)
lists each capability's status, and `chrome-cdp convert` translates simple
Playwright tests.

## Docs

- [A tour of chrome-cdp](docs/guide.md): everything it does, one area at a
  time.
- [Command reference](docs/commands/README.md): every command and flag.
- Guides: [locators and refs](docs/locators-and-refs.md),
  [waiting](docs/waiting.md), [debugging a dev server](docs/debugging-a-dev-server.md),
  [testing with `.cdp` files](docs/testing.md), [CI](docs/ci.md),
  [Docker](docs/docker.md), [security model](docs/security.md),
  [troubleshooting](docs/troubleshooting.md).
- [E2E testing](docs/e2e/README.md).
- [All docs](docs/README.md), and [development](docs/development.md) for
  working on chrome-cdp itself. [CHANGELOG.md](CHANGELOG.md) says what each
  release changed.

## Status

Pre-1.0: every command of the v1 surface is implemented, with
integration tests against real Chrome that CI runs on Linux and macOS,
x86_64 and arm64. The CLI syntax
and the `--json` output (`chrome-cdp schema <command>`) are the public API
and follow semver from 1.0. The design and roadmap live in
[docs/plans/](docs/plans/00-overview.md).
