<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp convert`

Translate a Playwright test file into a .cdp script (best effort)

```text
chrome-cdp convert [OPTIONS] <SPEC>
```

Arguments:

- `<SPEC>`: The Playwright test file, e.g. tests/login.spec.ts; `-` reads stdin

Options:

- `-o, --output <PATH>`: Write the script to this file instead of printing it
- `--max-chars <N>`: Bound the printed script to this many characters (0 = no limit) (default `20000`)

```text
Reads a Playwright test (TypeScript or JavaScript) and writes the .cdp
script that does the same, following docs/migrating-from-playwright.md:
goto, getBy… locators, click, fill, check, selectOption, press, waitFor…,
route with fulfill or abort, and the expect matchers. Each test becomes a
section after a `# test: …` comment, with its beforeEach steps and
test.use options; the tests run one after another in one session.

What it cannot translate (conditions, loops, helper functions, values kept
in variables) becomes a `# TODO line N (why): code` line, and a warning
counts them: rewrite those before running the script. Nothing is run and
no browser starts.

Examples:
  chrome-cdp convert tests/login.spec.ts
  chrome-cdp convert tests/login.spec.ts -o login.cdp && chrome-cdp run login.cdp
  chrome-cdp convert - < checkout.spec.ts > checkout.cdp
  chrome-cdp convert tests/login.spec.ts --json    # the script and each TODO
```

