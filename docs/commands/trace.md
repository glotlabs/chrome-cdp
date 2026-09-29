<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp trace`

Record a Chrome performance trace, for DevTools or Perfetto

```text
chrome-cdp trace [OPTIONS] <COMMAND>
```

```text
`trace start` starts Chrome's tracing on the tab (the Performance panel's
categories by default); `trace stop` saves the trace as JSON, which
DevTools' Performance panel (Load profile) and https://ui.perfetto.dev
open. For a quick look at a page's speed, `chrome-cdp perf` is cheaper.
Needs a running session.

Examples:
  chrome-cdp trace start --screenshots
  chrome-cdp reload && chrome-cdp trace stop -o load.json
  chrome-cdp trace stop -o trace.json.gz        # gzipped
```

## `chrome-cdp trace start`

Start a performance trace of the tab

```text
chrome-cdp trace start [OPTIONS]
```

Options:

- `--screenshots`: Also record screenshots of the page as it renders (the film strip)
- `--categories <LIST>`: Trace categories instead of the Performance panel's, comma-separated; `-name` leaves one out (`-*` all others)

```text
Starting while a trace runs changes nothing and says so.

Examples:
  chrome-cdp trace start
  chrome-cdp trace start --screenshots                     # with a film strip
  chrome-cdp trace start --categories devtools.timeline,v8
```

## `chrome-cdp trace stop`

Stop the trace and save it

```text
chrome-cdp trace stop [OPTIONS]
```

Options:

- `-o, --output <PATH>`: Where to save it (`.gz`: gzipped); a numbered file in the session's traces/ directory if not given

```text
Prints the file's path (-q: only the path); the trace goes to the
session's traces/ directory (1.json, 2.json, …) unless -o names a file. A
path ending in .gz is gzipped.

Examples:
  chrome-cdp trace stop -o trace.json
  chrome-cdp trace stop -o trace.json.gz
  chrome-cdp trace stop --timeout 2m          # a long trace takes a while
```

