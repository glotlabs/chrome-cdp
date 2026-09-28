# chrome-cdp

A single static binary that drives Chrome over the Chrome DevTools Protocol,
built first for AI agents working from a shell. It aims to replace Playwright
for agents: one command per step, compact output, refs instead of selectors,
and errors that say what to run next.

**Status:** early development. Sessions (`open`, `connect`, `status`,
`sessions`, `close`), tabs (`tabs`, `tab`), navigation (`goto`, `back`,
`forward`, `reload`), `wait`, `snapshot`, reading the page (`find`, `text`,
`html`, `attr`, `value`, `count`, `box`, `markdown`), the mouse (`click`,
`dblclick`, `hover`, `tap`, `drag`, `scroll`, `mouse`), the keyboard and
forms (`press`, `type`, `fill`, `select`, `check`, `uncheck`, `focus`,
`blur`), files and the clipboard (`upload`, `clipboard`), dialogs (`dialog`),
`downloads`, what the page logged (`console`, `errors`) and requested
(`network`), what it looks like (`screenshot`, `pdf`), JavaScript (`eval`),
recordings and measurements (`har`, `trace`, `perf`, `coverage`), helpers
for a human watching (`highlight`, `inspect`), raw CDP calls,
`install`/`uninstall` and `schema` work so far:

```sh
chrome-cdp install                    # Chrome for Testing, if there is no Chrome
chrome-cdp open https://example.com   # starts the session's browser
chrome-cdp goto example.com/about     # → 200 https://example.com/about "About" (412ms)
chrome-cdp snapshot -i                # what can be acted on, with refs: button "Save" [e6]
chrome-cdp find checkout              # - button "Checkout" [e6] (in form "Cart")
chrome-cdp click e6                   # clicked e6 button "Checkout"
chrome-cdp fill e9 'ada@example.com'  # filled e9 textbox "Email"
chrome-cdp press Enter                # pressed Enter in e9 textbox "Email"
chrome-cdp markdown                   # the article, as Markdown
chrome-cdp wait --text 'Welcome back'  # instead of sleep
chrome-cdp errors                     # exceptions and error logs not seen yet
chrome-cdp network --failed           # #42 POST 500 /api/login xhr 12KB 340ms
chrome-cdp screenshot --annotate      # …/shots/1.png: 1280x720 png, 48KB; refs labelled
chrome-cdp eval --in e9 'el.value'   # ada@example.com
chrome-cdp perf                       # load timing, LCP/CLS/INP rated, blocking time, heap
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
holds: an element (`wait '#results'`, `wait .spinner --state hidden`,
`wait e12 --state detached`; any locator, below), visible text (`--text`,
`--text-gone`, optionally `--in <locator>`), the URL (`--url /dashboard`, a glob or
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
A password field says `(password)` (its value is never shown) and a file
input `(file)`. Hidden content, and the page behind an open modal dialog,
is left out, a list is cut after 30 of a kind (`--all` shows
them), text is cut at 200 characters (`--text-limit`) and the output at
20,000 (`--max-chars`). `-i` keeps only what has a ref, with the landmarks,
tables and rows around it; `--scope e12` (or any locator, e.g.
`--scope frame=payment`) and `--depth 3` show a part; `--diff` shows what changed since the tab's last
snapshot (`+` added, `-` removed, `~` changed; after a page load it says
the nodes were compared by place). The same page gives the same
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

Every command that names an element takes the same locators:

| Form | Finds |
|---|---|
| `e12`, `@e12` | the element a snapshot gave that ref |
| `#id`, `.cls`, `[attr]`, `css=…` | CSS; a space also reaches into open shadow roots |
| `//…`, `xpath=…` | XPath |
| `text=Sign in`, `text="Sign in"`, `text=/sign.?in/i` | the innermost visible element with that text: a case-insensitive part, the whole text when quoted, or a regex |
| `role=button[name="Save"]` | an ARIA role and accessible name (quoted: exact), with `[checked]`, `[disabled]`, `[expanded]`, `[level=2]`, `[pressed]`, `[selected]`, `[include-hidden]` |
| `label=Email`, `placeholder=…`, `alt=…`, `title=…`, `testid=…` | by label, attribute, or test id (`data-testid`; `testid_attribute` in the config changes it) |
| `A >> B` | B inside A; `>> nth=0` (`-1` is the last), `>> visible=true`, `>> has-text=…` filter |
| `frame=payment >> #card` | inside an iframe, named by its name, a part of its URL, or its ref (`e8 >> #card` also works) |

A command that acts on one element fails with `Ambiguous` when several match,
and lists them with new refs and where each is (`e41: button "Delete" (in row
"Invoice 12 …")`), so the next command can use a ref. `wait`, `text` and
`count` take any number of matches.

Reading the page without a full snapshot:

- `find <words> [--role r] [--scope loc] [--limit 10]` searches roles, names,
  values and text (every word must match; a typo in a long word is forgiven)
  and prints the best matches as snapshot lines with refs, each with the
  named container it is in. `--scope` searches one part of the page (a
  dialog, a row, an iframe). `-q` prints only the refs.
- `text [loc]` prints the visible text of the page, or of every match (one
  per line), tidied; `html [loc] [--outer] [--raw]` the HTML, pretty-printed
  (open shadow roots included) unless `--raw`.
- `attr <loc> <name>`, `value <loc>` (a field's current value; a select's
  chosen value), `count <loc>` (`4 (3 visible)`) and `box <loc>`
  (`x=24 y=310 w=120 h=36` in CSS pixels of the tab's viewport, iframes
  included).
- `markdown [loc]` prints the page's main content as Markdown: a single
  `<main>` or `<article>`, else the element with the most paragraph text,
  without navigation, sidebars, footers and form controls.

`text`, `html` and `markdown` read the page without a locator, and a frame's
document when the locator names one (`text frame=payment`). An element that
is not there is `NotFound` (exit 1); an attribute that is not there, or an
element that is not rendered (for `box`), prints nothing and a warning.

Acting with the mouse: `click <loc>` waits (up to `--timeout`, 10s by
default) until the element is visible, stable, enabled and not covered,
scrolls it into view, and clicks its centre with real, trusted mouse
events; it prints what it clicked (`clicked e6 button "Sign in"`). When
something covers it, the error names what, with a ref, and a click that
likely dismisses it (`click e89   (button "Accept" inside it)`). `--button
right|middle`, `--count 2` (or `dblclick`), `--modifiers Shift,Meta`,
`--position x,y` (from the element's top-left corner) and `--force` (skip
the checks) change the click. `hover <loc>` moves the mouse over it, and
`tap <loc>` touches it when the tab emulates a touch screen.
`drag <from> <to> [--steps 10]` presses, moves and releases (what sortable
lists on mouse or pointer events need), and finishes an HTML5 drag the page
starts with real drag events. `scroll [loc] --by 0,600` or `--to
top|bottom|x,y` turns the wheel over the element or the page (infinite
scroll loads more) and prints where it is and which end it reached; `scroll
<loc>` alone scrolls it into view. `mouse move|down|up|wheel` sends raw
events at viewport points; a press is held across commands.

Typing: `fill <loc> <text>` is the fast path: it waits until the field is
visible, enabled and editable, selects what it holds and inserts the text
as one trusted edit, so React, Vue and Svelte state follows (`""` clears;
date, time, colour and range inputs get their value set). `type <text>
[--in loc] [--delay 50ms]` presses a key per character instead (every key
listener runs; emoji and CJK are inserted as an input method does).
`press <keys> [--in loc]` sends keys and chords where focus is: `Enter`,
`Shift+Tab`, `"Control+A Backspace"`; `Mod` is Meta when the browser runs
on macOS and Control elsewhere (also in `click --modifiers`). `select <loc>
<value|label>...` picks a `<select>`'s options, `check`/`uncheck <loc>`
click a checkbox, radio or switch only when needed (a hidden input through
its label) and make sure it changed, and `focus`/`blur <loc>` move focus.

Files: `upload <loc> <file>...` gives a file input (even a hidden one, or
the input a `<label>` names) its files, and the page gets `input` and
`change`; for a button that opens a file chooser, it clicks the button and
fills the chooser instead. `--clear` empties the input. `clipboard read`
and `clipboard write <text>` use the page's clipboard API (the page must be
on https or localhost, and keeps the clipboard permission): read what a
page's Copy button copied, or write text and `press Mod+V` to paste it.

After its input, every action waits for what it caused and says what
happened, so there is rarely a need to snapshot again. A navigation is
followed until the new page has loaded and settled; otherwise the action
waits briefly (at most 1.5s) for the requests it sent and for the DOM to go
quiet. Lines follow the action's own only when something happened:

```
clicked e6 button "Sign in"
→ navigated to https://acme.test/dashboard "Dashboard" (200)
! console error: TypeError: x is undefined (app.js:12)
! request failed: #42 POST /api/session (500)
! opened tab t2 (https://acme.test/help); switch to it: chrome-cdp tab t2
```

Also reported: `pushState` navigations (`(same document)`), dialogs the
session answered, downloads, and focus moving to another form (`! focus
moved to e14 textbox "Code"`). Failed requests count only on the page's own
origin; `#42` is the request's id for `network show 42`. `--json` lists them all in `events`. `--no-wait-after` returns once
the input is sent; `--snap` appends what changed on the page (`snapshot
--diff -i`), and `snap_after_action = "diff"` in the config files makes that
the default (`--no-snap` turns it off for one command).

JavaScript dialogs never hang a command. By default an `alert` and a
`beforeunload` ("leave the page?") are accepted and mentioned in the output;
a `confirm` or `prompt` stays open, and the command that ran into it returns
at once with exit code 5: `dialog open in tab t1: confirm "Delete item?"`.
`dialog accept [text]` or `dialog dismiss` answers it; `tabs` shows open
dialogs. `open --dialogs accept|dismiss|manual` changes the policy for the
session (`manual`: every dialog waits for `dialog`).

Every tab records its console from the moment it opens, so `errors` and
`console` work after the fact: `errors` shows the uncaught exceptions,
`console.error`s, CSP violations and failed loads from the page's own origin
that it has not shown before, each with the top of its stack; `console`
shows every message (`--level warn|error|debug`, `--grep`, `--since 0` for
all, `--since 30s`, `--follow`). Stack frames of localhost pages (or with
`--sourcemaps`) point at the original sources through their source maps:

```
#7 error uncaught TypeError: Cannot read properties of undefined (reading 'code')
    at e (/src/cart.ts:7:29)
    at /src/main.ts:7:3
```

Each tab keeps its newest 1,000 entries (`open --console-buffer N`), and
dedicated workers log into their tab's console. A wait that times out says
how many errors are unread.

The same goes for requests: `network` lists what the tab requested since it
last ran, one line each, and a request it showed while pending once more when
it ends. `--failed` keeps errors (no response, or a status of 400 or more)
and `--type xhr,fetch`, `--url '**/api/**'`, `--method` and `--status 4xx`
filter; `--follow` keeps printing requests as they come:

```
#41 GET 200 /api/items fetch 12KB 35ms
#42 POST 500 /api/login xhr 1.2KB 340ms
#43 GET failed http://localhost:4000/api fetch 4ms net::ERR_FAILED (CORS: MissingAllowOriginHeader)
```

`network show 42` prints one request in full (status, sizes, timing, what
sent it); `--headers`, `--body` (JSON pretty-printed; with `-q`, just the
body) and `--request-body` add the rest; `-o file` saves the response body
(an image as its bytes) and prints the path. Chrome keeps response bodies for a
while, also after the page navigates away, and the output says when it no
longer has one. Each tab keeps its newest 2,000 requests (`open
--network-buffer N`); `network clear` empties the log.

`screenshot` saves an image of the viewport, the whole page (`--full`) or one
element (`screenshot e12`, scrolled into view first) and prints its path:

```
/run/user/1000/chrome-cdp/default/shots/3.png: 1280x720 png, 48KB
```

Images go to the session's `shots` directory, numbered, unless `-o` names a
file (`-o -` writes it to stdout). They are in CSS pixels and at most 1,568
pixels on their longest side (`--max-size`, scaled down to fit), a size
vision models take without resizing, so a point in the image is a point on
the page. `--annotate` labels the elements `snapshot -i` gives refs with
those refs, so a model can map what it sees to what it can click; `--mask
<locator>` paints over elements (prices, dates) for stable images.
`--format jpeg|webp`, `--quality`, `--scale device` and `--omit-background`
do what they say. `pdf` prints the page to the session's `pdfs` directory
(or `-o`) with `--format A4|Letter|…`, `--landscape`, `--background`,
`--margin 1cm`, `--pages 1-3,5` and `--css-page-size` (the page's `@page`
size wins); Chrome prints only when headless.

`eval <js>` runs a script in the page like a line typed into DevTools'
console (statements, top-level `await`, a user gesture) and prints its value:
a string as it is, anything else as JSON, a DOM node as `button#save.primary`.
A promise is awaited and a function (`el => el.value`) called. `--in
<locator>` binds one element as `el`, `--frame` runs in a frame, `--world
isolated` away from the page's own scripts, and `eval -` reads the script from
stdin. An exception exits 1 with its message and stack, page frames resolved
with source maps on localhost:

```
error: the script threw TypeError: Cannot read properties of null (reading 'sum')
  at: explode (/src/app.ts:8:1)
  at: eval:1:1
```

`har start` and `har stop -o run.har` save the requests in between (of every
tab, or of `--tab`) as a HAR 1.2 file, response bodies included unless
`--content omit`; it is built from the network log, so nothing has to be
turned on in advance. `trace start [--screenshots]` and `trace stop -o
trace.json` record a Chrome performance trace (gzipped if the name ends in
`.gz`) that DevTools' Performance panel and Perfetto open. `perf` sums up
how fast the page is in a few lines:

```
t1 http://localhost:3000/ (loaded 4.2s ago)
load: TTFB 12ms · FCP 180ms · DOMContentLoaded 150ms · load 420ms · 14 requests, 230KB
LCP 610ms good (img#hero)
CLS 0.138 needs improvement (1 shift)
INP 256ms needs improvement (1 interaction; slowest: click on button#save)
TBT 200ms (1 long task)
page: 1,203 elements · 45 listeners · JS heap 4.1MB of 6MB
```

`coverage start` and `coverage stop` list the scripts and stylesheets by the
bytes that never ran or matched, as DevTools' Coverage panel does. For a
human watching a headed browser (`open --headed`), `highlight e12` points out
an element for two seconds (`--duration`), and `inspect [--panel console]`
opens DevTools for the tab.

`cdp` is the escape hatch to everything else: `cdp <Domain.method> [json]`
calls the browser (or a page, with `--target page`, `--tab` or `--frame`), and
`--listen <Domain.event|Domain.*>` also prints the events that follow, one per
line, until `--count` of them came (`--count 0`: all until `--timeout`).

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
`--session`, `--timeout`, `base_url` and `testid_attribute` can live in
`~/.config/chrome-cdp/config.toml` or a project's `.chrome-cdp.toml`.

## Development

```sh
cargo xtask ci      # fmt --check, gen-cdp --check, clippy -D warnings, tests
cargo xtask gen-cdp # regenerate CDP types (crates/cdp/protocol/README.md)
cargo xtask build-js # rebuild the injected helper from TypeScript (needs npx)
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

chrome-cdp's in-page helper (the locator engine and the DOM walker) is
TypeScript in `crates/engine/src/injected/src/`. `cargo xtask build-js`
typechecks it and bundles it with esbuild into `injected/ccdp.js`, which is
committed, so building chrome-cdp needs no Node; a test fails when the bundle
is older than its sources.

## Docs

The design and roadmap live in [docs/plans/](docs/plans/00-overview.md).
