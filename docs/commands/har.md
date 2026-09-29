<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp har`

Record the page's requests to a HAR file

```text
chrome-cdp har [OPTIONS] <COMMAND>
```

```text
`har start` marks where each tab's network log is; `har stop` writes the
requests since then, of every tab (or of --tab), as a HAR 1.2 file that
DevTools, Charles or a HAR viewer can open. Response bodies go in unless
`--content omit`. Needs a running session.

Examples:
  chrome-cdp har start
  chrome-cdp click e5 && chrome-cdp har stop -o checkout.har
  chrome-cdp har start --content omit --tab t2
```

## `chrome-cdp har start`

Start recording requests (of every tab, or of --tab)

```text
chrome-cdp har start [OPTIONS]
```

Options:

- `--content <HOW>`: Response bodies: in the file (`embed`) or left out (`omit`) (one of `embed`, `omit`; default `embed`)

```text
Starting again while recording keeps the recording and takes the new
--content.

Examples:
  chrome-cdp har start
  chrome-cdp har start --content omit      # sizes and types, no bodies
  chrome-cdp har start --tab t2
```

## `chrome-cdp har stop`

Save the requests recorded since `har start` to a HAR file

```text
chrome-cdp har stop [OPTIONS]
```

Options:

- `-o, --output <PATH>`: Where to save it; a numbered file in the session's hars/ directory if not given

```text
Prints the file's path (-q: only the path); the file goes to the session's
hars/ directory (1.har, 2.har, …) unless -o names one. Bodies Chrome no
longer has are left out, with a warning.

Examples:
  chrome-cdp har stop -o checkout.har
  chrome-cdp har stop -q
```

