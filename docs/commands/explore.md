<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp explore`

Map an app: follow its links (and buttons) from a start page, and list every page and state found, how to reach each, and what was wrong on each

```text
chrome-cdp explore [OPTIONS] [URL]
```

Arguments:

- `<URL>`: Where to start (relative to base_url); the active tab's page if left out

Options:

- `--max-states <N>`: Stop after finding this many states (default `30`)
- `--max-actions <N>`: Stop after this many clicks and loads, replayed ones included (default `200`)
- `--depth <N>`: Follow nothing more than this many steps from the start (default `3`)
- `--time <DURATION>`: Stop after this long, e.g. 30s, 5m (default `2m`)
- `--click-buttons`: Click buttons, tabs, menu items and disclosure controls too, not only links
- `--submit-forms`: Click buttons that submit a form
- `--allow-destructive`: Click controls whose name looks destructive (delete, log out, pay, …)
- `--origin <ORIGIN>`: Follow links to this origin too (repeatable)
- `--include <PATTERN>`: Follow only links to URLs this pattern matches (repeatable)
- `--exclude <PATTERN>`: Never follow links to URLs this pattern matches (repeatable)
- `--layout`: Record layout's error findings in each state
- `--screenshots`: With -o: save a screenshot of each state
- `-o, --output <DIR>`: Write map.json and states/&lt;id&gt;.cdp here (an empty or new directory)
- `--exit-code`: Exit 1 when a state had an error, a failed request, a broken link or a layout error
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Starts at URL (else the active tab's page) and walks breadth first:
on each state it clicks, in page order, the links to the start's origin
(and --origin's) that --include and --exclude (URL patterns, as wait
--url takes) allow, and with --click-buttons the buttons, tabs, menu
items and controls with aria-expanded or aria-haspopup. A state is the
URL (no fragment, query keys sorted, utm_* dropped) plus the page's
skeleton (its controls' roles and its headings', landmarks' and
dialogs' names), so an open modal is a state and a list with other data
is not. Clicks run the normal action code, so client-side routing runs;
to get back to a state it loads its URL when that shows the same
skeleton, else replays its path. A link to where another led already
is not clicked again.

It never submits a form (--submit-forms clicks submit buttons), types,
uploads, clicks a control whose name looks destructive (delete, remove,
log out, sign out, unsubscribe, pay, buy, checkout, confirm, reset, …;
--allow-destructive), leaves the allowed origins (other links are
listed as external), or follows mailto:, tel:, javascript:, download or
file links. alert and beforeunload are accepted, confirm and prompt
dismissed; a new tab a click opens is closed and its URL loaded
instead.

It runs in a context of its own with a copy of the current one's
cookies, storage, routes, mocks, init scripts and emulation, and closes
it at the end (also on Ctrl-C): your tabs are left alone. It stops at
--max-states (30), --max-actions (200, replayed steps included),
--depth (3 steps from the start) or --time (2m), and says which
stopped it and what was left. Each state lists its console errors,
failed requests and, with --layout, layout's errors.

-o DIR (empty or new) writes map.json (the --json data) and
states/<id>.cdp, a scenario that gets to each state (chrome-cdp run
DIR/states/s4.cdp), and with --screenshots states/<id>.png. It exits 0
whatever it finds; --exit-code exits 1 when a state had an error, a
failed request, a broken link or a layout error. -q prints how many
problems. Needs a running session.

Examples:
  chrome-cdp explore http://localhost:5173/
  chrome-cdp explore --click-buttons --layout --exit-code
  chrome-cdp explore /admin --include '**/admin/**' --max-states 50
  chrome-cdp explore http://localhost:5173/ -o map --screenshots
```

