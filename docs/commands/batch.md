<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp batch`

Run commands from stdin, one per line, in one call

```text
chrome-cdp batch [OPTIONS]
```

Options:

- `--continue`: Run every step even after one fails; the exit code is the first failure's
- `--var <KEY=VALUE>`: Set ${KEY} in the script; repeat for more

```text
Each line is a command as you would type it after `chrome-cdp`, with the
same quoting; `#` starts a comment. Every line is checked before the first
runs, then they run in order and stop at the first failure (--continue
runs the rest). Each step prints `[3/7] click e6 ✓` and its usual output;
with --json, one JSON line per step. Lines inherit --session, --timeout,
--tab, --json and -q. ${name} takes a --var, ${env:NAME} an environment
variable. The full syntax: docs/scenario-syntax.md.

Examples:
  chrome-cdp batch <<'EOF'
    goto localhost:3000/login
    fill label=Email ada@example.com
    fill label=Password "${env:APP_PASSWORD}"
    click 'role=button[name="Sign in"]'
    wait --url /dashboard
  EOF
  printf 'goto example.com\ntext h1\n' | chrome-cdp batch --json
  chrome-cdp batch --continue --var user=ada < steps.cdp
```

