<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp debug`

Log values at a source line each time it runs (logpoints), without stopping the page or editing its code

```text
chrome-cdp debug [OPTIONS] <COMMAND>
```

```text
`debug log FILE:LINE EXPR…` sets a logpoint in the tab's scripts: the line
of an original source file, found through the scripts' source maps (a
script without one by its own URL). Each time the line runs, the page
logs the expressions' values without stopping: in `console`, and in the
report of the action that ran it (`! logpoint 1 src/cart.ts:42: total=12`).
It is set again after a reload, and in scripts loaded later. `debug list`
shows the logpoints and their hits; `debug clear` removes them and turns
the debugger off when nothing else needs it. While the debugger is on, a
`debugger;` statement does not stop the page: it is resumed and noted in
the console.

`debug locals on|off|localhost` says where uncaught errors keep their
frames' local variables for `errors --locals` (localhost pages by default;
the debugger is on there for them). Needs a running session.

Examples:
  chrome-cdp debug log src/cart.ts:42 total items.length
  chrome-cdp click e12                   # ! logpoint 1 src/cart.ts:42: total=12, items.length=3
  chrome-cdp debug list
  chrome-cdp debug clear --all
  chrome-cdp debug locals off
```

## `chrome-cdp debug log`

Set a logpoint: log expressions' values each time a source line runs

```text
chrome-cdp debug log [OPTIONS] <FILE:LINE> [EXPR]...
```

Arguments:

- `<FILE:LINE>`: The source line: FILE:LINE or FILE:LINE:COLUMN (1-based), e.g. src/cart.ts:42
- `<EXPR>`: JavaScript expressions to log, each one argument; `-` reads one from stdin

Options:

- `--limit <N>`: Stop logging after this many hits (default `1000`)

```text
FILE is matched by path against the original sources of the tab's scripts
(`src/cart.ts` matches `http://localhost:5173/src/cart.ts` and
`webpack://app/./src/cart.ts`); when it names several, the error lists
them and more of the path picks one. A line with no code uses the next
line with code, and says so. A file no script has yet is pending: the
logpoint binds when a script with it loads (a lazy chunk, a reload).

Each EXPR is one JavaScript expression, run in the scope of the line
(before it runs) and shown with its text as the label; one that throws
shows its error in its place. Values are previewed in the page: 2 levels
deep, strings cut at 80 characters, collections over 5 entries counted.
`-` reads one expression from stdin. A session has at most 20 logpoints.
-q prints the logpoint's id.

Examples:
  chrome-cdp debug log src/cart.ts:42 total 'items.length'
  chrome-cdp debug log src/cart.ts:42:7 item
  chrome-cdp debug log src/App.tsx:30             # was the line reached?
  chrome-cdp debug log src/cart.ts:42 total --limit 5
```

## `chrome-cdp debug list`

List the logpoints, where they are and how often they were hit

```text
chrome-cdp debug list [OPTIONS]
```

```text
One line per logpoint: its id, place, expressions and hits, and whether it
is pending (no script has its file yet) or stopped (it reached --limit).
-q prints the ids.

Examples:
  chrome-cdp debug list
  chrome-cdp debug list --json
```

## `chrome-cdp debug clear`

Remove a logpoint, or all of them

```text
chrome-cdp debug clear [OPTIONS] [ID]
```

Arguments:

- `<ID>`: The logpoint's id (`debug list` shows them)

Options:

- `--all`: Remove every logpoint

```text
When no logpoint is left, the debugger is turned off in the tab, unless
locals capture keeps it on (see `debug locals`).

Examples:
  chrome-cdp debug clear 1
  chrome-cdp debug clear --all
```

## `chrome-cdp debug locals`

Say where uncaught errors keep their local variables, or change it

```text
chrome-cdp debug locals [OPTIONS] [on|off|localhost]
```

Arguments:

- `<on|off|localhost>`: on, off or localhost; none to show the setting (one of `on`, `off`, `localhost`)

```text
Where locals capture is on, the page pauses at each uncaught error (a
thrown error or a rejected promise nobody handles) for up to 200ms while
chrome-cdp reads the local, closure and block variables of its top 3
frames (10 each at most, previewed like logpoint values), then resumes.
`errors --locals` and `console --locals` show them. At most 10 errors a
second are read; the rest say so.

  on         every page, iframe and worker of the session
  off        none
  localhost  documents and workers on this machine (the default)

With no argument, it says where it is on. -q prints the setting.

Examples:
  chrome-cdp debug locals
  chrome-cdp debug locals on      # a staging site too
  chrome-cdp debug locals off
```

