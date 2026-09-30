# A tour of chrome-cdp

This page walks through everything chrome-cdp does, one area at a time.
The [command reference](commands/README.md) has every flag, and the
guides go deeper: [locators and refs](locators-and-refs.md),
[waiting](waiting.md), [debugging a dev server](debugging-a-dev-server.md),
[testing with `.cdp` files](testing.md) and [troubleshooting](troubleshooting.md).

A taste, one step per command:

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
chrome-cdp route '**/api/cart' --json-body '{"items":[]}'  # mock an API for every tab
chrome-cdp emulate 'iPhone 15'        # viewport, pixel ratio, touch and user agent
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

## Sessions and the browser daemon

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

## Tabs

Tabs have short ids (`t1`, `t2`, …) in the order they open, never reused in a
session. Commands act on the active tab, or on the one `--tab t2` (or
`CHROME_CDP_TAB`) names. `tab t2` switches, `tab new [url]` opens one and
switches to it, and `tab close [id]` closes one; closing the last tab leaves a
blank one unless `--close-browser` closes the session. A popup (`window.open`,
`target=_blank`) opens a new tab that stays in the background, and the command
that opened it says `opened tab t3`; `open --follow-popups` makes popups
active instead. The active tab stays in front, so its page stays visible
(`document.visibilityState`), with animation frames, full-speed timers and
geolocation; the other tabs are hidden, as background tabs are. Every new tab, frame and worker is set up before it runs any
script.

## Navigation

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

## Frames

Each tab's frames are tracked, cross-site (out-of-process) iframes included.
`chrome-cdp frames` lists them as a tree, in the order of their `<iframe>`
elements, with the tab's workers (dedicated, shared and service).
`cdp --frame <name|url-part|id>` sends a call to one:
`chrome-cdp cdp --frame checkout Runtime.evaluate '{"expression":"location.href"}'`
runs in that iframe's page, whichever process it lives in. chrome-cdp's own
scripts run in an isolated world in each frame, out of the page's reach.

## Waiting

[waiting.md](waiting.md) explains all of this in more depth.

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

## Snapshots

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

## Refs

A node keeps its ref while it stays in the page, also when the page
re-renders around it, and a ref number is never given to another node. When
the page (or an iframe) loads a new document, its refs go stale: using one
fails with `RefStale`. When a re-render replaced the node, the ref moves to
the one element of its frame with the same role and name and says so
(`warning: e6 re-resolved by role+name`); with none, or several, it fails
with `RefDetached` and what it was (`was: button "Save"`). `--strict-refs`
(or `CHROME_CDP_STRICT_REFS=1`) turns the role+name fallback off, and so does
a route change (`pushState`) for refs shown before it.

## Locators

[locators-and-refs.md](locators-and-refs.md) has the whole story of
refs and locators.

Every command that names an element takes the same locators:

| Form | Finds |
|---|---|
| `e12`, `@e12` | the element a snapshot gave that ref |
| `#id`, `.cls`, `[attr]`, `css=…` | CSS; a space also reaches into open shadow roots |
| `//…`, `xpath=…` | XPath |
| `text=Sign in`, `text="Sign in"`, `text=/sign.?in/i` | the innermost visible element with that text: a case-insensitive part, the whole text when quoted, or a regex |
| `role=button[name="Save"]` | an ARIA role and accessible name (quoted: exact), with `[checked]`, `[disabled]`, `[expanded]`, `[level=2]`, `[pressed]`, `[selected]`, `[include-hidden]` |
| `label=Email`, `placeholder=…`, `alt=…`, `title=…`, `testid=…` | by label, attribute, or test id (`data-testid`; `testid_attribute` in the config changes it) |
| `A >> B` | B inside A; `>> nth=0` (`-1` is the last), `>> visible=true`, `>> has-text=…`, `>> has-not-text=…` filter |
| `A >> has=B`, `A >> has-not=B` | the elements of A with (without) a descendant B finds ([more](locators-and-refs.md#inner-locators-has-has-not-and-or)) |
| `A >> and=B`, `A >> or=B` | the elements of A that B also finds; A's plus B's, in page order |
| `frame=payment >> #card` | inside an iframe, named by its name, a part of its URL, or its ref (`e8 >> #card` also works) |

A command that acts on one element fails with `Ambiguous` when several match,
and lists them with new refs and where each is (`e41: button "Delete" (in row
"Invoice 12 …")`), so the next command can use a ref. `wait`, `text` and
`count` take any number of matches. `locator e12` prints the locator a
script or test should use for a ref's element.

## Reading the page

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

## The mouse

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

## The keyboard and forms

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

## Files and the clipboard

Files: `upload <loc> <file>...` gives a file input (even a hidden one, or
the input a `<label>` names) its files, and the page gets `input` and
`change`; for a button that opens a file chooser, it clicks the button and
fills the chooser instead. `--drop` drops the files on the element with
drag events instead, for a drop zone with no file input: the page reads
them from the drop's `dataTransfer.files`, and `upload` fails if it takes
no drop there. `--clear` empties the input. `clipboard read`
and `clipboard write <text>` use the page's clipboard API (the page must be
on https or localhost, and keeps the clipboard permission): read what a
page's Copy button copied, or write text and `press Mod+V` to paste it.
`clipboard write --html '<b>Bold</b>'` puts rich text there (with its
plain text), `--file chart.png` an image; `clipboard read --type
text/html` prints the HTML and `--type image/png -o copied.png` saves the
image.

## What an action reports

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
session answered, downloads, and focus moving to another form or into
another frame (`! focus moved to e14 textbox "Code"`, `… in iframe "pay"`). Failed requests count only on the page's own
origin; `#42` is the request's id for `network show 42`. `--json` lists them all in `events`. `--no-wait-after` returns once
the input is sent; `--snap` appends what changed on the page (`snapshot
--diff -i`), and `snap_after_action = "diff"` in the config files makes that
the default (`--no-snap` turns it off for one command).

## Dialogs

JavaScript dialogs never hang a command. By default an `alert` and a
`beforeunload` ("leave the page?") are accepted and mentioned in the output;
a `confirm` or `prompt` stays open, and the command that ran into it returns
at once with exit code 5: `dialog open in tab t1: confirm "Delete item?"`.
`dialog accept [text]` or `dialog dismiss` answers it (waiting up to
`--timeout` for one to open, so a prompt that follows a confirm needs no
`wait`); `tabs` shows open dialogs. `open --dialogs accept|dismiss|manual` changes the policy for the
session (`manual`: every dialog waits for `dialog`).

## Console and errors

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

## Network

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

## Mocking and blocking requests

`route <pattern>` mocks, blocks or changes requests whose URL matches (a glob
such as `**/api/*`, a `/regex/`, or a part of the URL), in every tab, popup
and frame of the browser context (the session's own unless `--context`), the ones opened later too, before the page's first
script runs. A rule has one action: answer (`--json-body`, `--body`,
`--body-file`, `--status`, `--header`), `--abort [reason]`, change
(`--set-header`, `--remove-header`, `--rewrite-url`), or `--patch-json`,
which lets the real response through with a JSON merge patch applied ("what
if the API returned an empty list"). `--method`, `--resource-type`, `--times
N` and `--delay 2s` narrow and shape it. The first rule that matches, in the
order they were added, handles a request; others go on untouched:

```sh
chrome-cdp route /api/login --method POST --status 401 --json-body '{"error":"bad password"}'
chrome-cdp route '**/api/cart' --patch-json '{"items": [], "total": 0}'
chrome-cdp route /api/ --status 503 --times 1   # fail once, then the real server
chrome-cdp block --images --fonts --ads          # faster loads
chrome-cdp routes                                # r1 POST /api/login → fulfill 401 application/json 24B (2 hits)
chrome-cdp routes --log                          # each request routing saw, and the rule that handled it
chrome-cdp unroute r1                            # or --all
```

Requests are intercepted only while routes exist. Meanwhile they skip service
workers (which would answer before the routes) and the HTTP cache; `open
--bypass-service-worker` skips service workers without routes too. A mocked
response to another origin allows the page's origin, and CORS preflights for
it are answered, so mocking a cross-origin API just works.

## Emulation

Emulation shapes the environment pages run in. Like routes, every setting
belongs to the browser context (the session's own unless `--context`):
it applies to every tab, popup and frame in it, the ones
opened later too, before the page's first script runs, and stays until it
is reset. In headless mode pages get a fixed 1280x800 viewport until one is
set (a `--chrome-arg=--window-size=…` of yours wins; headed, the window
decides).

```sh
chrome-cdp emulate 'iPhone 15'          # a device: viewport, pixel ratio, mobile, touch, user agent
chrome-cdp emulate --list               # 45 phones, tablets and desktops (from Playwright's table)
chrome-cdp open --device 'Pixel 7' localhost:3000   # the same, before the first page loads
chrome-cdp viewport 1440x900 --dpr 2    # or 390x844 --mobile --touch; `viewport reset`
chrome-cdp geo 59.9139,10.7522          # navigator.geolocation (the permission is granted)
chrome-cdp tz Europe/Berlin             # time zone; `locale de-DE` sets Intl and Accept-Language
chrome-cdp ua 'MyBot/1.0'
chrome-cdp media --color-scheme dark --reduced-motion reduce   # also --type print, --forced-colors active
chrome-cdp offline on                   # requests fail, navigator.onLine is false
chrome-cdp throttle slow-3g             # fast-3g, 4g, or --latency 300ms --down 1000 --up 500
chrome-cdp cpu-throttle 4
chrome-cdp time freeze 2026-01-31T09:30:00Z   # Date is frozen; `time advance 90s`, `time reset`
chrome-cdp time pause                   # timers and animation frames stop too; `time run-for 1s`, `time resume`
chrome-cdp permissions grant notifications camera --origin http://localhost:3000
chrome-cdp emulate                      # what the session emulates; `emulate reset` turns it all off
```

Each command has a `reset` (or `off`). `time freeze` replaces `Date` (and
`Intl.DateTimeFormat` without a date) with an init script; timers and
animations keep running in real time, since Chrome's virtual time stalls
page loads. `time pause` stops them as well: `setTimeout`, `setInterval`
and `requestAnimationFrame` callbacks wait and `performance.now()` stands
still until `time run-for 1s` fires them in order (each interval tick, a
frame every 16ms, `ran 10 timers, 62 frames`), `time pause-at T` runs on
to a time, or `time resume` lets them run in real time again. Pause before
the page loads: timers it set before the clock came keep running. Permissions (`permissions`, and the ones `geo` and `clipboard`
grant) belong to the browser: `permissions` lists them, `permissions reset`
takes them back. `open --fake-media` gives pages a fake camera and
microphone. Out-of-process iframes get every setting but the pixel ratio,
which Chrome sets only on the page.

## Init scripts and exposed functions

Init scripts run in every new document of the session, before the page's
own scripts: stub an API, set a feature flag, or record what the page does.
Like routes and emulation they belong to the browser context and reach every tab,
popup and frame. `expose` gives pages a function whose calls land in the
tab's console, so a page (or an init script) can report back:

```sh
chrome-cdp init-script add ./stubs.js           # s1; runs from the next load (`reload`), or --now
chrome-cdp init-script add --code 'window.FEATURE_X = true' --world isolated
chrome-cdp init-script list                     # s1 main /home/me/app/stubs.js 2KB
chrome-cdp init-script remove s1                # or --all
chrome-cdp expose onCheckout                    # window.onCheckout(...) → console: binding onCheckout({"total":42})
chrome-cdp unexpose onCheckout                  # or --all
```

A script is checked in the active tab when it is added, and one that does
not compile is refused. Its errors name its file (or `init-script-s1.js`) at
its own line numbers. `--world isolated` runs it where `eval --world
isolated` runs: the same DOM, out of the page's reach. An exposed function
takes any arguments (sent as JSON) and returns a promise.

## Cookies, storage and saved logins

Cookies are the browser's, so they belong to every tab; Web Storage belongs
to a page's origin. `state save` writes both to a file in Playwright's
`storageState` format, so a login done once can be loaded into a new
session, and Playwright's auth files load as they are:

```sh
chrome-cdp cookies                              # session=ab12  app.example.com/  HttpOnly Secure session
chrome-cdp cookies set theme=dark               # for the active tab's URL, or --url/--domain
chrome-cdp cookies clear --domain example.com   # or --name, --url, or everything
chrome-cdp storage local                        # localStorage of https://app.example.com: 2 items
chrome-cdp storage session set draft 7          # get, set, remove, clear; --origin for an iframe's
chrome-cdp state save auth.json                 # cookies + storage of the visited origins (--indexeddb)
chrome-cdp open --state auth.json https://app.example.com   # logged in from the first load
```

A state file holds secrets (session cookies, tokens): it is written readable
by you only; keep it out of version control. `state load` adds the cookies
and gives each origin its storage, at once if a tab has it open, else before
the scripts of its next page load. chrome-cdp adds `sessionStorage` to the
format (Playwright ignores it). For a whole persistent profile instead, use
`open --profile <name>`.

## Screenshots and PDFs

`screenshot` saves an image of the viewport, the whole page (`--full`) or one
element (`screenshot e12`, scrolled into view first) and prints its path:

```
/run/user/1000/chrome-cdp/default/shots/3.png: 1280x720 png, 48KB
```

Many apps never scroll the page itself: `html, body { height: 100% }` and a
scrolling `<main>`. There `--full` finds the container the page scrolls in
(the scrolling element that shows the most of itself, if that is at least a
quarter of the viewport), makes the viewport tall enough for all of its
content for the capture, and then puts the viewport and the container's
scroll position back. The output names it (`the page scrolls in main#feed`;
`container` in `--json`). `--full '#feed'` names the container when the one
found is not the one you want.

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

## JavaScript

`eval <js>` runs a script in the page like a line typed into DevTools'
console (statements, top-level `await`, a user gesture) and prints its value:
a string as it is, anything else as JSON, a DOM node as `button#save.primary`
and an element of the page with a ref to act on, `button#save.primary [e14]`
(the first 50 elements of a value; a warning says how many more got none).
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

## Recordings and performance

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
bytes that never ran or matched, as DevTools' Coverage panel does, cross-site
iframes and workers included (each such file marked with where it ran). For a
human watching a headed browser (`open --headed`), `highlight e12` points out
an element for two seconds (`--duration`), and `inspect [--panel console]`
opens DevTools for the tab.

## Raw CDP

`cdp` is the escape hatch to everything else: `cdp <Domain.method> [json]`
calls the browser (or a page, with `--target page`, `--tab` or `--frame`), and
`--listen <Domain.event|Domain.*>` also prints the events that follow, one per
line, until `--count` of them came (`--count 0`: all until `--timeout`).
`--all-sessions` listens on every session of the tab, its cross-site iframes'
and workers' too (and those that start meanwhile), and starts each line with
what sent it: `[main]`, `[iframe "pay"]`, `[service worker sw.js]`.

## Downloads, crashed tabs and HTTP authentication

Downloads go to the session's `downloads` directory, named as the server
suggests (`report (1).pdf` if the name is taken). `goto` to a file waits for
it and prints `downloaded: report.pdf (12KB) → /path/report.pdf`, a command
that starts one says so, `wait --download` waits for one to finish, and
`downloads` lists them. A tab whose page crashed fails commands with exit
code 4 until `reload` (or `goto`) loads it again. `open --http-credentials
user:password` (or `-` to read them from stdin) answers HTTP basic and digest
authentication.

## Finding and installing Chrome

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

## Launch options, profiles and extensions

Chrome runs headless with its sandbox on and a temporary profile that is
deleted when the session closes. `open --headed` opens a window,
`--profile <name>` keeps logins between runs, `--user-data-dir <path>` picks
the directory, and `--chrome-arg=<flag>` adds a Chrome flag; `open --restart`
applies new ones to a running session. Chrome never outlives its session
daemon, even `kill -9`. [chrome-flags.md](chrome-flags.md) lists the flags and
explains the sandbox and profile rules. `open --extension <dir>` (repeatable)
loads an unpacked extension, headless too, before the first page; `open`
and `status` print its id, so `goto chrome-extension://<id>/popup.html`
opens its pages. It works in Google Chrome as well as Chrome for Testing,
but not in chrome-headless-shell.

## Output, exit codes and config

Every command prints its result on stdout and notes and errors on stderr,
takes `--json` (one `{"ok":…}` object) and `-q`, and exits 0 ok, 1 action
failed, 2 usage, 3 timeout, 4 browser/daemon, 5 dialog. `--timeout` defaults
to 10 s for `wait` and 30 s for everything else. Defaults for
`--session`, `--timeout`, `base_url` and `testid_attribute` can live in
`~/.config/chrome-cdp/config.toml` or a project's `.chrome-cdp.toml`.
[output.md](output.md) has the whole contract and what stays stable.

## Scripts

[testing.md](testing.md) shows scripts and tests in use.

`batch` runs many commands in one call, one per line from stdin, and
`run login.cdp --var email=ada@example.com` does the same from a file, with
`${name}`, `${env:NAME}` and `${data:users.json#/0/email}` filled in. Every
line is checked before the first one runs, and each step prints
`[3/7] click e6 ✓` and its usual output (`--json`: one line per step); the
first failure stops the script unless `--continue`. The syntax is specified
in [scenario-syntax.md](scenario-syntax.md):

```sh
chrome-cdp batch <<'EOF'
goto localhost:3000/login
fill label=Email ada@example.com
fill label=Password "${env:APP_PASSWORD}"
click 'role=button[name="Sign in"]'
wait --url /dashboard
EOF
```

## Coming from Playwright

Coming from Playwright? [migrating-from-playwright.md](migrating-from-playwright.md)
maps its API (locators, actions, waits, `expect`, mocking, contexts) to
commands, and `chrome-cdp convert tests/login.spec.ts -o login.cdp`
translates a simple test file into a scenario for `chrome-cdp test`, with a
`# TODO` line for each statement it cannot translate.

## E2E tests and mock files

For E2E tests of a web frontend against a mocked backend
([e2e/README.md](e2e/README.md) has the full docs), `chrome-cdp init e2e`
scaffolds a project: an `[e2e]` section in `chrome-cdp.toml` whose web server and `base_url` come from `package.json`
(Vite, Next.js, Create React App, Astro, SvelteKit), an example scenario in
`e2e/scenarios/`, a mock set in `e2e/mocks/`, and a GitHub Actions workflow.
It never overwrites a file. `chrome-cdp help config` lists the `[e2e]` keys.

The mock sets in `e2e/mocks/` are plain TOML: each `[[mock]]` matches
requests (method, path with `:params` and globs, query, headers, a JSON
subset, form, body, GraphQL) and answers them (JSON inline or from a data
file, text, files, a network error, or the real server with a patch), with
templates such as `{{params.id}}`, `{{json.email}}`, `{{uuid}}` and
`{{now}}`. A set's `scope` says which requests it owns; an owned request no
mock answers gets a 501 that says so. `mock use` applies sets to the session,
on top of the project's `default_mocks`, so the app can be explored by hand
against its mocks:

```sh
chrome-cdp mock use checkout                     # default_mocks, then checkout
chrome-cdp mock list                             # the sets and mocks, with hits
chrome-cdp mock log --unmatched                  # requests no mock answered
chrome-cdp mock explain GET /api/cart?user=7     # which mock answers, why the others don't
chrome-cdp mock off
chrome-cdp schema mocks                          # the file format's JSON Schema
```

Mock files are checked when they are applied, with each problem at
`file:line:col`, and read again when they change. `route` rules come before
any mock.

Mocks can keep state, so flows like "create, then list" or "log in, then
load the profile" work without code. A `sequence` answers differently on
each call (busy once, then ok). `[state]` declares named values that mocks
require (`when = { logged_in = true }`) and change (`set_state`). A
`[[collection]]` is a fake REST resource seeded from a data file: `GET`
lists it, `POST` adds to it, and `GET`/`PUT`/`PATCH`/`DELETE` work on
`/:id`:

```toml
[[collection]]
name = "todos"
path = "/api/todos"
seed = "data/todos.json"
```

```sh
chrome-cdp mock state todos                      # the collection's items now
chrome-cdp expect mock-state todos count 3       # retried until the page's POST lands
chrome-cdp mock seed todos data/many-todos.json  # other items
chrome-cdp mock reset                            # counters, sequences, state and collections start over
```

## Help

`chrome-cdp --help` fits on one screen: the commands most tasks need, by
task, and the locator grammar. `help commands` lists every command, `help
<command>` shows one in full (each has examples), and `help
locators|refs|waiting|errors|config|scripts` explain the ideas behind them.
A mistyped command, flag, key, device or topic gets a "did you mean" and a
hint with the corrected command line.

## For agents

For agents, `chrome-cdp skill` prints a `SKILL.md` (Claude Code and other
Agent Skills clients) that teaches the workflow, the usual pitfalls and a
cheat sheet in about 1,100 tokens. `skill --install` writes it to the
project's `.claude/skills/chrome-cdp/`, `skill --install --user` to
`~/.claude/skills/chrome-cdp/`. `skill --format agents-md` prints a section
for `AGENTS.md`, and `skill --format llms-txt` an `llms.txt`. The skill's
source is [crates/chrome-cdp/src/help/SKILL.md](../crates/chrome-cdp/src/help/SKILL.md);
the agent benchmark installs it with `skill --install`, so what it
measures is what an agent reads. `completions
bash|zsh|fish|elvish|powershell` prints a shell completion script.
