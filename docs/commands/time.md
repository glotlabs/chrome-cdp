<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp time`

Freeze or pause the time pages see, move it on, or give the real one back

```text
chrome-cdp time [OPTIONS] <COMMAND>
```

```text
While frozen, `Date` (and `Intl.DateTimeFormat` without a date) gives the
frozen time in every tab, frame and worker, from each page's (or worker's)
first script on; a warning says how many workers already running got it
late.
Timers and animations keep running in real time. `time advance` also moves
performance.now() on and runs the setTimeout and setInterval callbacks due
by then (each once), so expiry, polling and debounce can be tested without
waiting. It is an init script that replaces `Date`, not Chrome's virtual
time, which stalls page loads.
`time pause` stops timers and animation frames as well, and `time run-for`
fires them in order, each interval tick and a frame every 16ms, like
Playwright's clock.runFor.

Examples:
  chrome-cdp time freeze 2026-01-31T09:30:00Z
  chrome-cdp time advance 90s
  chrome-cdp time pause
  chrome-cdp time run-for 1s
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
not run early. While paused (`time pause`), it fires each callback due by
then once, where `time run-for` fires every interval tick and frame.

Examples:
  chrome-cdp time advance 90s
  chrome-cdp time advance 2h
  chrome-cdp time advance 1500        # ms
```

## `chrome-cdp time pause`

Freeze the time and stop timers and animation frames

```text
chrome-cdp time pause [OPTIONS] [TIME]
```

Arguments:

- `<TIME>`: ISO 8601, e.g. 2026-01-31T09:30:00Z, or ms since 1970

```text
TIME is ISO 8601 (UTC unless it has an offset) or ms since 1970; without it,
the frozen time, or now. While paused, setTimeout, setInterval and
requestAnimationFrame callbacks are queued and performance.now() stands
still, until `time run-for`, `pause-at` or `resume`. Timers a page set
before the clock came to it keep running: reload for full control.

Examples:
  chrome-cdp time pause
  chrome-cdp time pause 2026-01-31T09:30:00Z
  chrome-cdp time run-for 5s
```

## `chrome-cdp time run-for`

Move the paused time on, firing every callback due, in order

```text
chrome-cdp time run-for [OPTIONS] <DURATION>
```

Arguments:

- `<DURATION>`: How far: 1500 (ms), 90s, 15m, 2h, 7d

```text
Each interval tick fires (100ms over 1s: 10 times), animation frames run
every 16ms, and callbacks set on the way run when they are due. It stops
after 10,000 callbacks, leaving the rest queued. `time advance` while
paused fires each due callback only once.

Examples:
  chrome-cdp time run-for 1s
  chrome-cdp time run-for 15m
  chrome-cdp time run-for 500        # ms
```

## `chrome-cdp time pause-at`

Run the paused time on to TIME, as run-for, then stay paused

```text
chrome-cdp time pause-at [OPTIONS] <TIME>
```

Arguments:

- `<TIME>`: ISO 8601, e.g. 2026-01-31T10:00:00Z, or ms since 1970

```text
TIME is ISO 8601 (UTC unless it has an offset) or ms since 1970, and not
before the paused time.

Examples:
  chrome-cdp time pause-at 2026-01-31T10:00:00Z
```

## `chrome-cdp time resume`

Let paused timers run in real time again; the time stays frozen

```text
chrome-cdp time resume [OPTIONS]
```

```text
Examples:
  chrome-cdp time resume
  chrome-cdp time reset              # the real time, too
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

