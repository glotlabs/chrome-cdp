<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp coverage`

Measure how much of the page's JavaScript and CSS is unused

```text
chrome-cdp coverage [OPTIONS] <COMMAND>
```

```text
`coverage start` turns on V8's block coverage and CSS rule tracking in the
tab, its cross-site iframes and its workers (and those that start while it
runs); `coverage stop` lists the scripts and stylesheets by unused bytes
(code that never ran, rules that never matched), as DevTools' Coverage
panel does. Inline scripts and styles are summed per document; a file that
ran in a cross-site iframe or a worker is marked [iframe "pay"] or
[dedicated worker w.js]. Needs a running session.

Examples:
  chrome-cdp coverage start
  chrome-cdp reload && chrome-cdp click e4 && chrome-cdp coverage stop
  chrome-cdp coverage stop --limit 5 --json
```

## `chrome-cdp coverage start`

Start measuring the tab's JavaScript and CSS coverage

```text
chrome-cdp coverage start [OPTIONS]
```

```text
Starting while it measures changes nothing and says so. Load or reload the
page after starting to count its startup code.

Examples:
  chrome-cdp coverage start
  chrome-cdp coverage start --tab t2
```

## `chrome-cdp coverage stop`

Stop measuring and list the files by unused bytes

```text
chrome-cdp coverage stop [OPTIONS]
```

Options:

- `--limit <N>`: List at most this many files, most unused bytes first (0 = all) (default `20`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
-q prints one line per file: unused, total and URL, tab-separated.

Examples:
  chrome-cdp coverage stop
  chrome-cdp coverage stop --limit 0          # every file
```

