<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp expect-snapshot`

Compare the page's snapshot with a stored text baseline; a diff shows what changed

```text
chrome-cdp expect-snapshot [OPTIONS] <NAME>
```

Arguments:

- `<NAME>`: The baseline's name: letters, digits, -, _ and ., with / for folders (login, checkout/cart)

Options:

- `--scope <LOCATOR>`: Only this element's subtree (any locator)
- `--full`: Every node (as `snapshot --all` shows them), not only what `snapshot -i` shows
- `--update`: Store the page as the baseline instead of comparing
- `--dir <DIR>`: The directory of the baselines, instead of the project's .chrome-cdp/snapshots
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit); also bounds a failure's diff (default `20000`)

```text
Reads the active tab (or --tab) as `snapshot -i` shows it, without refs
(and long lists in full), and compares it with the baseline
.chrome-cdp/snapshots/<NAME>.txt, reading again until they match or
--timeout (default 5s) passes. A failure exits 1 (ExpectFailed) with a
unified diff: - lines are the baseline's, + lines the page's.

--update stores the page as the baseline instead, once two reads in a row
are alike. A first run with no baseline stores it too, unless the CI
environment variable is set: then a missing baseline fails. Commit the
files. They live in .chrome-cdp/ of the nearest directory up with
.chrome-cdp.toml, .chrome-cdp/ or .git (else the current one); --dir puts
them elsewhere. Needs a running session.

Examples:
  chrome-cdp expect-snapshot login
  chrome-cdp expect-snapshot checkout/cart --scope 'role=main'
  chrome-cdp expect-snapshot login --update
  chrome-cdp expect-snapshot home --full --timeout 10s
```

