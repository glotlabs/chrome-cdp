# chrome-cdp

A single static binary that drives Chrome over the Chrome DevTools Protocol,
built first for AI agents working from a shell. It aims to replace Playwright
for agents: one command per step, compact output, refs instead of selectors,
and errors that say what to run next.

**Status:** early development. Sessions (`open`, `connect`, `status`,
`sessions`, `close`), tabs (`tabs`, `tab`), navigation (`goto`, `back`,
`forward`, `reload`), `wait`, `snapshot`, dialogs (`dialog`), `downloads`,
raw CDP calls, `install`/`uninstall` and `schema` work so far:

```sh
chrome-cdp install                    # Chrome for Testing, if there is no Chrome
chrome-cdp open https://example.com   # starts the session's browser
chrome-cdp goto example.com/about     # → 200 https://example.com/about "About" (412ms)
chrome-cdp snapshot -i                # what can be acted on, with refs: button "Save" [e6]
chrome-cdp wait --text 'Welcome back'  # instead of sleep
chrome-cdp back
chrome-cdp cdp --target page Runtime.evaluate '{"expression":"document.title"}'
chrome-cdp tabs                       # * t1  https://example.com/  "Example Domain"
chrome-cdp status
chrome-cdp close
chrome-cdp schema open      # JSON Schema of `chrome-cdp open --json`
```

Each session (`--session <name>`, default `default`) has a background daemon
that keeps its browser warm between commands; the first command that needs a
browser starts it. The daemon closes the browser and exits after 30 minutes
without commands (`open --idle-timeout 2h`, `0` for never), on `close`, or if
killed, taking Chrome with it. `connect 9222` attaches a session to a Chrome
you started with `--remote-debugging-port=9222` instead; `close` then leaves
that browser running. Session files (socket, logs, `meta.json`) live in
`$XDG_RUNTIME_DIR/chrome-cdp/<session>` on Linux and
`$TMPDIR/chrome-cdp/<session>` on macOS; `status` shows the path and
`status --logs` the end of Chrome's log.

Tabs have short ids (`t1`, `t2`, …) in the order they open, never reused in a
session. Commands act on the active tab, or on the one `--tab t2` (or
`CHROME_CDP_TAB`) names. `tab t2` switches, `tab new [url]` opens one and
switches to it, and `tab close [id]` closes one; closing the last tab leaves a
blank one unless `--close-browser` closes the session. A popup (`window.open`,
`target=_blank`) opens a new tab that stays in the background, and the command
that opened it says `opened tab t3`; `open --follow-popups` makes popups
active instead. Every new tab, frame and worker is set up before it runs any
script.

`goto <url>` loads a page and waits for its load event and then for the page
to settle: no requests for 500 ms and no DOM changes for 300 ms, 5 s at most
(`--wait load|domcontentloaded|networkidle|commit|none` for less), then prints
the HTTP status, the final URL after redirects, the title and the time taken. A URL
without a scheme gets `https://` (`http://` for localhost and IPs), and a
relative one (`/login`, `./next`, `?q=1`) resolves against `base_url` from the
config, else the current page. A 4xx/5xx status is a warning; a page that
cannot load fails with a reason and a hint ("connection refused on
localhost:3000 — is your dev server running?"). `back` and `forward` walk the
tab's history (saying so at either end), and `reload [--hard]` loads the page
again. `open --ignore-https-errors` lets a session load sites with
self-signed certificates.

Each tab's frames are tracked, cross-site (out-of-process) iframes included.
`cdp --frame <name|url-part|id>` sends a call to one:
`chrome-cdp cdp --frame checkout Runtime.evaluate '{"expression":"location.href"}'`
runs in that iframe's page, whichever process it lives in. chrome-cdp's own
scripts run in an isolated world in each frame, out of the page's reach.

`wait` replaces `sleep`. It takes one condition and returns as soon as it
holds: an element (`wait '#results'`, `wait .spinner --state hidden`; CSS or
XPath until snapshot refs land), visible text (`--text`, `--text-gone`,
optionally `--in <locator>`), the URL (`--url /dashboard`, a glob or
`/regex/`), `--load`, `--idle` (network idle, not counting WebSockets,
beacons and requests open over 10 s), `--settle`, a JavaScript expression
(`--fn 'window.app.ready'`), or a request or response since the previous
command (`--response '**/api/save' --status 200`), or a download since the
previous command (`--download`). On timeout (exit 3) it says what it saw
instead: how many elements matched, the requests still in flight, the URL,
the last value, and the page.

`snapshot` shows the page as an accessibility tree, one node per line
(`- role "name" [ref] (hN) value="…" states… → href`), with a ref (`e12`) on
everything an agent can act on: controls, links, elements that only act like
buttons (`clickable`: `onclick`, `tabindex`, `cursor: pointer`), scrollable
boxes and iframes, whose content (cross-site too) is shown inside them.
Hidden content is left out, a list is cut after 30 of a kind (`--all` shows
them), text is cut at 200 characters (`--text-limit`) and the output at
20,000 (`--max-chars`). `-i` keeps only what has a ref, with the landmarks,
tables and rows around it; `--scope e12` (or a CSS/XPath locator) and
`--depth 3` show a part; `--diff` shows what changed since the tab's last
snapshot (`+` added, `-` removed, `~` changed). The same page gives the same
output. It reads Chrome's own accessibility tree; `--engine dom` uses
chrome-cdp's DOM walker instead.

A node keeps its ref while it stays in the page, also when the page
re-renders around it, and a ref number is never given to another node. When
the page (or an iframe) loads a new document, its refs go stale: using one
fails with `RefStale`. When a re-render replaced the node, the ref moves to
the one element of its frame with the same role and name and says so
(`warning: e6 re-resolved by role+name`); with none, or several, it fails
with `RefDetached` and what it was (`was: button "Save"`). `--strict-refs`
(or `CHROME_CDP_STRICT_REFS=1`) turns the role+name fallback off, and so does
a route change (`pushState`) for refs shown before it.

JavaScript dialogs never hang a command. By default an `alert` and a
`beforeunload` ("leave the page?") are accepted and mentioned in the output;
a `confirm` or `prompt` stays open, and the command that ran into it returns
at once with exit code 5: `dialog open in tab t1: confirm "Delete item?"`.
`dialog accept [text]` or `dialog dismiss` answers it; `tabs` shows open
dialogs. `open --dialogs accept|dismiss|manual` changes the policy for the
session (`manual`: every dialog waits for `dialog`).

Downloads go to the session's `downloads` directory, named as the server
suggests (`report (1).pdf` if the name is taken). `goto` to a file waits for
it and prints `downloaded: report.pdf (12KB) → /path/report.pdf`, a command
that starts one says so, `wait --download` waits for one to finish, and
`downloads` lists them. A tab whose page crashed fails commands with exit
code 4 until `reload` (or `goto`) loads it again. `open --http-credentials
user:password` (or `-` to read them from stdin) answers HTTP basic and digest
authentication.

It finds Chrome by itself: `--chrome <path>`, then `$CHROME_CDP_CHROME`, then
browsers installed in the chrome-cdp cache (`~/.cache/chrome-cdp`, or
`~/Library/Caches/chrome-cdp` on macOS; `$CHROME_CDP_CACHE` overrides it),
then a system Google Chrome or Chromium. `status` shows which one it used.

`chrome-cdp install` downloads [Chrome for
Testing](https://googlechromelabs.github.io/chrome-for-testing/) into that
cache: `--channel stable|beta|dev|canary`, `--version 154`, `--headless-shell`
for the smaller headless-only build, `--force` to download again, and `--list`
for what is installed and available. `chrome-cdp uninstall <id>` removes one.
On Linux it checks that the browser starts; if system libraries are missing it
prints the `apt-get`/`dnf` line, and `--with-deps` runs it (with sudo, asking
first). On Ubuntu 23.10+, `--with-deps` also writes the AppArmor profile that
Chrome's sandbox needs there. `--auto-install` (or `CHROME_CDP_AUTO_INSTALL=1`)
installs Chrome when none is found. `CHROME_CDP_CFT_URL` points at a mirror of
the Chrome for Testing index.

Chrome runs headless with its sandbox on and a temporary profile that is
deleted when the session closes. `open --headed` opens a window,
`--profile <name>` keeps logins between runs, `--user-data-dir <path>` picks
the directory, and `--chrome-arg=<flag>` adds a Chrome flag; `open --restart`
applies new ones to a running session. Chrome never outlives its session
daemon, even `kill -9`. [docs/chrome-flags.md](docs/chrome-flags.md) lists the flags and
explains the sandbox and profile rules.

Every command prints its result on stdout and notes and errors on stderr,
takes `--json` (one `{"ok":…}` object) and `-q`, and exits 0 ok, 1 action
failed, 2 usage, 3 timeout, 4 browser/daemon, 5 dialog. `--timeout` defaults
to 10 s for `wait` and 30 s for everything else. Defaults for
`--session`, `--timeout` and `base_url` can live in
`~/.config/chrome-cdp/config.toml` or a project's `.chrome-cdp.toml`.

## Development

```sh
cargo xtask ci      # fmt --check, gen-cdp --check, clippy -D warnings, tests
cargo xtask gen-cdp # regenerate CDP types (crates/cdp/protocol/README.md)
cargo xtask dist    # release binary in dist/ (size budget: 15 MB)
cargo xtask fixtures # serve testdata/site on two local origins
cargo deny check    # licenses and advisories
```

Integration tests need a real Chrome: set `CHROME_PATH` to a Chrome or
Chromium binary. They load pages from the fixture site in `testdata/site`,
served by `crates/fixtures` on two sites (`127.0.0.1` and `localhost`, so
frames from one in the other run out of process) and on an HTTPS origin with
a self-signed certificate; `cargo xtask fixtures`
serves it for poking at by hand. Command output is pinned by golden files in
`crates/chrome-cdp/tests/golden/`; after an intended change, rewrite them with
`UPDATE_GOLDEN=1 cargo test -p chrome-cdp --test golden`. The snapshot corpus
(`testdata/site/snapshot/`: pages from five lines to 50,000 nodes, each with
an `.expected` tree) is checked by `cargo test -p engine --test snapshot`,
which also prints both engines' scores and output tokens; its golden outputs
live in `crates/engine/tests/snapshots/`.

## Docs

The design and roadmap live in [docs/plans/](docs/plans/00-overview.md).
