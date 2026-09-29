<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp run`

Run a .cdp script: commands one per line, with ${var} substitution

```text
chrome-cdp run [OPTIONS] <FILE>
```

Arguments:

- `<FILE>`: The script, e.g. login.cdp

Options:

- `--continue`: Run every step even after one fails; the exit code is the first failure's
- `--var <KEY=VALUE>`: Set ${KEY} in the script; repeat for more

```text
The same syntax as `batch` (docs/scenario-syntax.md), read from a file.
${name} takes a --var, ${env:NAME} an environment variable, and
${data:file.json#/a/0} a value from a JSON file next to the script.
Errors name the place: flow.cdp:12:5.

Examples:
  chrome-cdp run login.cdp
  chrome-cdp run login.cdp --var email=ada@example.com --var plan=pro
  chrome-cdp run smoke.cdp --continue --session ci
  chrome-cdp run login.cdp --json    # one JSON line per step
```

