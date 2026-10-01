<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp lint`

Check scenario, mock and audit baseline files for mistakes without running them

```text
chrome-cdp lint [OPTIONS] [PATH]...
```

Arguments:

- `<PATH>`: Files or folders; the project's E2E files if none

Options:

- `--format <FORMAT>`: How to print the problems: text, json (as --json) or github (workflow annotations) (one of `text`, `json`, `github`; default `text`)
- `--openapi <FILE>`: Check the mocks against this OpenAPI spec (JSON or YAML) instead of the config's `openapi`
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Without paths, checks the project's E2E files, as fmt finds them. Every
problem has a place (file:line:col), a severity, a rule and a hint.

Scenario rules: syntax (and directive misuse), unknown-command,
unknown-flag, bad-args, unknown-directive, unknown-device,
unknown-backend, locator (an invalid locator), include-missing,
include-cycle, include-param (a ${name} the @include line does not
pass), undefined-var (a ${var:x} missing from a backend's vars, an
unknown ${saved.x}, a missing data file), ref (e12 refs are
session-only; write a stable locator), duplicate-test, and the warnings
mock-only (mock state/seed/ws send/sse send or expect mock-state in a test that
real backends run), mock-literal (a value from the mock data, in a test
real backends run), sleep (wait <ms>) and no-expect (a test that checks
nothing). Mock files get the `mock check` rules (unreachable,
collection-shadowed, openapi-…), and audit baselines (the .json files of
e2e/audit and .chrome-cdp/audit) the rule audit-baseline (not valid JSON,
a field `schema audit-baseline` does not have, a bad key or version).
Environment variables are not needed: ${env:…} and ${secret:…} are not
read.

Errors exit 1; warnings alone exit 0. --format github prints workflow
annotations, so problems show inline on pull requests; --format json is
--json. `chrome-cdp test` runs these checks first and refuses to start
on errors.

Examples:
  chrome-cdp lint
  chrome-cdp lint e2e/scenarios/checkout.cdp
  chrome-cdp lint --format github    # in a GitHub workflow
  chrome-cdp lint --json
```

