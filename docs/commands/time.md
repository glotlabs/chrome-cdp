<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp time`

Freeze the time pages see, move it on, or give the real one back

```text
chrome-cdp time [OPTIONS] <COMMAND>
```

```text
While frozen, `Date` (and `Intl.DateTimeFormat` without a date) gives the
frozen time in every tab and frame, from each page's first script on.
Timers and animations keep running in real time. `time advance` also moves
performance.now() on and runs the setTimeout and setInterval callbacks due
by then (each once), so expiry, polling and debounce can be tested without
waiting. It is an init script that replaces `Date`, not Chrome's virtual
time, which stalls page loads.

Examples:
  chrome-cdp time freeze 2026-01-31T09:30:00Z
  chrome-cdp time advance 90s
  chrome-cdp time reset
```

## `chrome-cdp time freeze`

Freeze the time pages see (default: now)

```text
chrome-cdp time freeze [OPTIONS] [TIME]
```

Arguments:

- `<TIME>`: ISO 8601, e.g. 2026-01-31T09:30:00Z, or ms since 1970

```text
TIME is ISO 8601 (UTC unless it has an offset) or ms since 1970.

Examples:
  chrome-cdp time freeze
  chrome-cdp time freeze 2026-01-31T09:30:00Z
  chrome-cdp time freeze 2026-12-24T18:00+01:00
```

## `chrome-cdp time advance`

Move the frozen time on, running the timers due by then

```text
chrome-cdp time advance [OPTIONS] <DURATION>
```

Arguments:

- `<DURATION>`: How far: 1500 (ms), 90s, 15m, 2h, 7d

```text
Timers set before the time was frozen in a page that was already open are
not run early.

Examples:
  chrome-cdp time advance 90s
  chrome-cdp time advance 2h
  chrome-cdp time advance 1500        # ms
```

## `chrome-cdp time reset`

Give pages the real time back

```text
chrome-cdp time reset [OPTIONS]
```

```text
Examples:
  chrome-cdp time reset
```

