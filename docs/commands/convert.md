<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp convert`

Translate a Playwright test file into a .cdp scenario (best effort)

```text
chrome-cdp convert [OPTIONS] <SPEC>
```

Arguments:

- `<SPEC>`: The Playwright test file, e.g. tests/login.spec.ts; `-` reads stdin

Options:

- `-o, --output <PATH>`: Write the scenario to this file instead of printing it
- `--max-chars <N>`: Bound the printed scenario to this many characters (0 = no limit) (default `20000`)

```text
Reads a Playwright test (TypeScript or JavaScript) and writes the .cdp
scenario that does the same, following docs/migrating-from-playwright.md:
goto, getBy… locators, click, fill, check, selectOption, press, waitFor…,
route with fulfill or abort, and the expect matchers. Each test(…) becomes
a @test "describe > title" block (docs/e2e/scenarios.md), beforeEach and
afterEach become @before and @after, beforeAll and afterAll @before-all
and @after-all, test.skip/fixme/only @skip and @only, and test.use a
@viewport or @device setting plus emulation commands, and { tag } @tags.
chrome-cdp test runs it, each test in a fresh browser context. A file
without tests (a script that launches the browser itself) becomes a plain
script for run.

Values known now are read: the file's constants (texts, numbers, lists and
objects), for…of over such a list, and its helper functions, inlined
where they are called. What it cannot translate (conditions, helpers from
other files, values only known at run time) becomes a `# TODO line N
(why): code` line, and a warning counts them: rewrite those before running
it. A test left with no steps gets @skip, so the file still loads. Nothing
is run and no browser starts.

Examples:
  chrome-cdp convert tests/login.spec.ts
  chrome-cdp convert tests/login.spec.ts -o login.cdp && chrome-cdp test login.cdp
  chrome-cdp convert - < checkout.spec.ts > checkout.cdp
  chrome-cdp convert tests/login.spec.ts --json    # the scenario and each TODO
```

