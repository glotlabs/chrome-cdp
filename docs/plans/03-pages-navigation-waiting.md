# Plan 03 — Pages, navigation & waiting

Status: TODO
Depends on: 02

## Goal

Build reliable tab, frame and navigation management, plus a wait engine that
removes the need for `sleep`. This layer is what makes agents succeed on real
sites: SPAs, redirects, popups, iframes and slow networks.

## Milestone 3.1 — Target & tab manager

- Call `Target.setDiscoverTargets` and `Target.setAutoAttach
  {autoAttach: true, waitForDebuggerOnStart: true, flatten: true}` at the
  browser level, and again recursively on each page session. This catches
  popups, OOPIFs and workers before they run any script. For each new session:
  enable the needed domains, apply session-wide settings (emulation, routes,
  init scripts from plan 07), then call `Runtime.runIfWaitingForDebugger`.
  Settings are applied before any page JS runs; that is why this matters.
- Tabs get **stable short ids** (`t1`, `t2`, …) in creation order, never
  reused within a session. One tab is *active*; commands target it unless
  `--tab <id>` is given.
- A popup (`window.open`, `target=_blank`) opens a new tab. It does **not**
  become active by default. The action output says `opened tab t3 (url)`, so the
  agent can switch. `--follow-popups` makes new tabs active automatically.
- Commands: `tabs` (id, active marker, title, url), `tab new [url]`, `tab <id>`
  (activate; `Target.activateTarget` so headed mode shows it), `tab close [id]`.
- Closing the last tab closes the session only with `--close-browser`. By
  default, a blank tab stays so the session lives.

## Milestone 3.2 — Frames & execution contexts

- Track the frame tree per tab (`Page.frameAttached/Navigated/Detached`, plus
  OOPIF sessions). Map each frame to its main-world execution context (from
  `Runtime.executionContextCreated` with `auxData.isDefault`).
- Create one isolated world per frame (`Page.createIsolatedWorld`,
  name `__chrome_cdp`) for our helper scripts. Page JS can't see or tamper with
  it. Re-create it on navigation.
- Frames are addressed by locator scoping (plan 04):
  `frame=<name|url-substring|ref> >> ...`. The snapshot already inlines iframe
  content with refs, so agents rarely need this.

## Milestone 3.3 — Navigation

- `goto <url> [--wait load|domcontentloaded|networkidle|commit|none] [--timeout]`.
  Default wait: `load`, then a short *settle* (see 3.4). URLs without a scheme
  get `https://`. `localhost`/IPs get `http://`. A relative path resolves
  against `base_url` from the config (plan 08 §4) when it is set, else
  against the current URL. `file://` paths are allowed. `open <url>` uses
  the same navigation code as `goto`.
- Handle `Page.navigate` returning `errorText` (DNS failure, connection refused,
  certificate error). Each is a `NavigationFailed` error (exit 1, per plan 08
  §3) with a distinct `details.reason` and its own hint. Examples:
  "connection refused on localhost:3000 — is your dev server running?".
  `--ignore-https-errors` is a launch/session option
  (`Security.setIgnoreCertificateErrors`).
- Report the HTTP status of the main document. A 4xx/5xx is a **warning**, not
  an error, and appears in the output.
- `back`, `forward` (`Page.getNavigationHistory` +
  `navigateToHistoryEntry`; at either end, say so rather than failing), and
  `reload [--hard]`.
- Detect same-document navigations (`navigatedWithinDocument`, pushState). They
  count as navigations for waits and ref invalidation (plan 04).
- Output: `→ 200 https://final.url/after/redirects "Page Title" (412 ms)`.

## Milestone 3.4 — Wait engine

Build one primitive, `wait_until(condition, timeout, poll)`. It is
event-driven where possible and polls with `requestAnimationFrame` otherwise.
Everything else uses it.

Conditions:

- `--load` / `--domcontentloaded`: lifecycle events per frame
  (`Page.lifecycleEvent`).
- `--idle` (network idle): no more than 0 in-flight requests for 500 ms,
  ignoring long-lived connections (WebSocket, EventSource, and requests open
  longer than 10 s such as long-polls). This is more useful than Playwright's
  `networkidle`, which hangs on analytics beacons.
- `--settle`: network idle **and** the DOM is quiet (a MutationObserver sees no
  mutations for 300 ms, max 3 s). It runs by default after navigations and
  after actions that trigger navigation.
- `wait <locator> [--state visible|hidden|attached|detached|enabled]`.
- `wait --text "Welcome"` (visible text anywhere, or scoped with `--in <loc>`),
  and `wait --text-gone "Loading"`.
- `wait --url <glob|/regex/>`.
- `wait --fn '<js expression>'` (truthy; polled in the page).
- `wait --request <pattern>` / `--response <pattern> [--status 200]`.
- `wait <ms>` exists but prints a nudge toward an event-based condition.

On timeout, the error includes **what was observed**. Example: "waited 10 s
for text 'Welcome'; page is https://x/login, title 'Sign in', 2 console errors
(run `chrome-cdp errors`)". This is the single most useful thing for agent
debugging.

Default timeouts: 30 s for navigation, 10 s for waits and actions. Configure
per command with `--timeout` or per session with `CHROME_CDP_TIMEOUT`. So the
global `--timeout` has no fixed clap default (today it is 30 s for every
command): when neither the flag, the env var nor a config file sets it, each
command supplies its own default.

## Milestone 3.5 — Dialogs & lifecycle edge cases

- `Page.javascriptDialogOpening`: by default, `alert` is auto-accepted and
  logged, `beforeunload` is auto-accepted, and `confirm`/`prompt` **block**.
  The pending action returns right away with `dialog open: confirm "Delete
  item?" — run chrome-cdp dialog accept|dismiss [text]`. `--dialogs
  accept|dismiss|manual` sets the session policy.
- Downloads: `Browser.setDownloadBehavior` to `<session dir>/downloads`.
  Actions that trigger a download report `downloaded: file.pdf (123 KB) → path`.
  `downloads` lists them. `wait --download` waits for one.
- Renderer crash (`Inspector.targetCrashed`): report it, and `reload`
  recovers.
- Auth challenges (HTTP basic): `--http-credentials user:pass` via
  `Fetch.authRequired`.

## Tests

Milestone 3.1 builds the fixture server: one shared, in-process server for
`testdata/site` (for example in a test-support crate or module) that every
integration test and `cargo xtask fixtures` use. It serves two origins (two
ports) so cross-origin iframes run out of process. Later milestones and plans
add pages and routes to it instead of starting their own servers.

The fixture site (`testdata/site`) needs pages for: redirect chains, slow
responses (`?delay=`), 404/500, SPA pushState routing, lazily-rendered content,
endless analytics beacons, WebSocket keep-alive, popups, nested same-origin and
cross-origin iframes (serve on two ports or hostnames so OOPIFs actually
occur; use `--site-per-process`), alert/confirm/prompt/beforeunload, file
download, and a basic-auth route.

- Each wait condition has a positive case, a timeout case, and a test that
  checks the timeout error's contents.
- `--idle` returns on the beacon page and the WebSocket page within 2 s.
- A popup is detected as a new tab, with init scripts applied before its first
  script runs (assert that a global set by the init script is visible to the
  page's inline script).

## Acceptance criteria

- [ ] `goto` + default settle gives a correct snapshot on the SPA fixture with
      no extra wait, 50 runs in a row.
- [ ] Every navigation error type maps to a distinct `details.reason` and
      hint (all exit 1, `NavigationFailed`, per plan 08 §3).
- [ ] Cross-origin iframes are attached and addressable.

## Decisions log

_(append here)_
