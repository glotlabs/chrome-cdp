<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp fmt`

Format .cdp scenario files and mock files in place; --check only reports

```text
chrome-cdp fmt [OPTIONS] [PATH]...
```

Arguments:

- `<PATH>`: Files or folders; the project's E2E files if none

Options:

- `--check`: Change nothing: exit 1 and print a diff if a file is not formatted
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Without paths, formats the project's E2E files: every .cdp file below the
project root (chrome-cdp.toml's folder) and the mock files in the mocks
directory ([e2e] mocks, else e2e/mocks). A folder means the .cdp files
below it, and its .toml files when they are mock files.

.cdp files: 2-space bodies, one blank line before each @test and hook,
single quotes unless a word needs ${…} or a ' (directive arguments such
as test names take double quotes), long flags instead of short ones,
settings in a stable order (@skip @only @mocks @tags @backend @safe
@timeout @retries @viewport/@device @allow-console-errors), trailing
comments lined up. Mock files: the keys of each [[mock]] in the order
name, match, respond, sequence, …, `key = value` spacing, inline tables
kept inline. Comments are kept, and the meaning never changes: a file
whose formatted form would read differently is left alone, with an error.

--check changes nothing: it exits 1 and prints a diff when a file is not
formatted, for CI.

Examples:
  chrome-cdp fmt
  chrome-cdp fmt e2e/scenarios/checkout.cdp
  chrome-cdp fmt --check        # in CI, before chrome-cdp lint and test
```

