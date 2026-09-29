<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp expect-screenshot`

Compare a screenshot with a stored PNG baseline, pixel by pixel (experimental)

```text
chrome-cdp expect-screenshot [OPTIONS] <NAME> [LOCATOR]
```

Arguments:

- `<NAME>`: The baseline's name: letters, digits, -, _ and ., with / for folders (home, checkout/cart)
- `<LOCATOR>`: Capture only this element (any locator)

Options:

- `--full <LOC>`: The whole page, not only the viewport; with LOC, all of the container the page scrolls in (as screenshot --full does)
- `--mask <LOCATOR>`: Paint over the elements this locator matches (repeatable)
- `--threshold <T>`: How far (0 to 1) a pixel's colour may move before it has changed (default `0.1`)
- `--max-diff-pixels <N>`: How many pixels may change and still pass (default `0`)
- `--update`: Store the page as the baseline instead of comparing
- `--dir <DIR>`: The directory of the baselines, instead of the project's .chrome-cdp/screenshots
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Takes a screenshot of the active tab's viewport (or the --full page, or the
element LOCATOR names) in CSS pixels and compares it with the baseline
.chrome-cdp/screenshots/<NAME>-<os>.png, taking it again until it matches
or --timeout (default 5s) passes. A pixel has changed when its colour moved
more than --threshold (0 to 1); up to --max-diff-pixels may change. A
failure exits 1 (ExpectFailed) and saves the screenshot and a diff image
(changes in red) where `screenshot` saves its files.

Experimental: fonts render differently on each OS (so each OS has its own
baseline) and may change with Chrome's version. expect-snapshot is the
sturdier check. For the capture the caret is hidden, animations and
transitions are finished and endless ones stopped at their start (as
Playwright does). --mask paints over what changes on every load (clocks,
ads). --update, a first run, CI and --dir work as in expect-snapshot.
Needs a running session.

Examples:
  chrome-cdp expect-screenshot home
  chrome-cdp expect-screenshot logo 'role=img[name="Acme"]'
  chrome-cdp expect-screenshot dashboard --full --mask '.clock'
  chrome-cdp expect-screenshot home --update
```

