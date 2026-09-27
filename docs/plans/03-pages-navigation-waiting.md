# Plan 03 — Pages, navigation & waiting

Status: IN PROGRESS (3.1-3.3 done)
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
- [x] Every navigation error type maps to a distinct `details.reason` and
      hint (all exit 1, `NavigationFailed`, per plan 08 §3).
- [x] Cross-origin iframes are attached and addressable.

## Decisions log

_(append here)_
- 3.1: The fixture server is the `fixtures` crate (hyper 1, already in the tree through reqwest). Its two origins are `http://127.0.0.1:<a>` and `http://localhost:<b>`: different *sites*, not just ports, so a frame from one in the other is an OOPIF with Chrome's default site isolation; no `--site-per-process` is needed. Pages may use `{{ORIGIN_A}}`, `{{ORIGIN_B}}` and `{{ORIGIN}}`, any path takes `?delay=<ms>`, and responses are `no-store`. Tests use `Fixtures::shared()` (one per test process, on its own thread). `cargo xtask fixtures` runs the crate's binary, since xtask stays std-only.
- 3.1: A new target's setup steps (auto-attach, `Page.enable`, init scripts) and `Runtime.runIfWaitingForDebugger` are all sent before any answer is awaited. Chrome runs them in order, but `Page.enable` in a noopener popup's new renderer only answers once the target runs, so awaiting each step first stalled every such popup for the 10 s step timeout and skipped its init scripts. To make "sent in order" hold, `cdp::Session::call` now queues the request when called, not when first polled.
- 3.1: Tab ids are given in the order Chrome announces the targets (`attachedToTarget`), but a tab is listed and usable only once its setup has finished. Pages with a `subtype` (prerender) are not tabs; other target kinds (workers, `browser_ui`, extension pages) are attached and resumed but not listed.
- 3.1: When the active tab closes, the tab that was active before it becomes active, else the newest tab. When no tab is left (a page ran `window.close()`), the next page command opens a blank one.
- 3.1: "Opened" tabs are the ones no command created, not only those with an `openerId` (Chrome sets it for noopener popups too; a user's Ctrl+T in headed mode counts as well). A page command reports them as typed `events` (`tab_opened`, the first `protocol::schema::Event`) and, because `cdp`'s stdout is the raw JSON result, as a warning line (`opened tab t2 (url); switch to it: chrome-cdp tab t2`). The report waits up to 500 ms for the popup's first URL. A popup that attaches after the command returned (a `target=_blank` click is asynchronous) is not reported; plan 05's post-action settle should catch those for actions.
- 3.1: `--follow-popups` is an `open` flag that sets the session's policy (kept until changed, and across a daemon upgrade); it is not a launch option, so it never forces `--restart`. `--tab` is a global flag with `CHROME_CDP_TAB`, used by `cdp --target page`, `open <url>` and `tab close`.
- 3.1: `tabs`, `tab <id>` and `tab close` need a running session and do not start one (`DaemonUnavailable`, hint `chrome-cdp open`); `tab new` starts one like `cdp` does. `tab` with no id or subcommand prints its help. The plan 08 checklist test now also checks a command that has both subcommands and positionals (`tab`).
- 3.1: `Engine::add_init_script` applies a script to every current page and OOPIF session and to all later ones (also after a browser restart). It is the hook for plan 07's init scripts; only tests call it for now. Emulation and routes add their steps to `tabs::setup`, and 3.2 adds frame tracking there.
- 3.1: `status` and `sessions` count tabs from the manager instead of calling `Target.getTargets`.
- 3.2: The frame tree lives in `engine::frames` (one `FrameTree` per tab, fed by the tab's and its OOPIF sessions' `Page`/`Runtime` events in `tabs::watch_host`). Events of different sessions arrive in different tasks, so the tree tolerates their order: a `frameDetached` with reason `swap` keeps the frame, and the last session to report it navigating owns it. On `frameNavigated` the frame's old children are dropped, because Chrome does not always send `frameDetached` for them (a reload kept a stale same-site child). `Page.getFrameTree`, sent during setup, only adds frames not already known from events.
- 3.2: The isolated world is added with `Page.addScriptToEvaluateOnNewDocument {source: "", worldName: "__chrome_cdp", runImmediately: true}` during each page/OOPIF session's paused setup, instead of calling `Page.createIsolatedWorld` after every navigation: Chrome then makes the world in every new document before page scripts run, with no race after a navigation. `createIsolatedWorld` is only a fallback for a frame that has a main world but no isolated one yet; Chrome keeps one world per name and frame, so both give the same world. Universal access is not granted.
- 3.2: Setup now also sends `Runtime.enable` to every page and OOPIF session (needed for `executionContextCreated`). Plan 06's console capture gets its events from the same call.
- 3.2: Frames are named by their id, else an iframe whose `name` is the query, else one whose URL contains it; the main frame only by id. Several matches are `Ambiguous` and none is `NotFound` (exit 1), and both list the tab's frames as notes and in `details.frames`. Plan 04's `frame=` locator reuses `FrameTree::find` and adds refs.
- 3.2: The milestone has no command of its own, so two entry points make frames usable now: `Engine::evaluate(tab, frame, world, js)` (the hook for plan 06's `eval --frame --world` and plan 04's injected helpers) and `cdp --frame <name|url-part|id>`, which sends a raw call to the session that owns the frame and fills in the frame's main-world context for `Runtime.evaluate`, `callFunctionOn`, `compileScript` and `globalLexicalScopeNames` when the params name none. A call that fails with "Cannot find context with specified id" (it never ran; the frame navigated) is retried in the new context, up to 3 times; "Execution context was destroyed" is not retried, since the script may have run.
- 3.3: A navigation follows the tab's main frame through its own events (`navigate::watch`), subscribed before the call is sent. Page sessions now get `Page.setLifecycleEventsEnabled` during setup; `--wait load|domcontentloaded|networkidle|commit` are the lifecycle events `load`, `DOMContentLoaded`, `networkIdle` and `init` of the awaited document. Only documents the watch expects (the loader `Page.navigate` returned) or saw start (`Page.frameStartedNavigating`) count, plus back/forward-cache restores: Chrome commits a failed navigation's error page just after `Page.navigate` answers, and taking that commit for the next navigation's made it race Chrome (`net::ERR_FAILED`). A client-side redirect that commits before the wait is met is followed.
- 3.3: `--wait networkidle` is Chrome's own `networkIdle` (no requests for 500 ms) until 3.4 builds the idle tracker that ignores WebSockets and long-polls. The default wait is `load` without a settle: 3.4 adds the settle after the wait in `Engine::navigate_page`.
- 3.3: The HTTP status comes from the Navigation Timing entry (`responseStatus`), so no `Network` domain is needed yet (plan 06 turns it on for its buffers). It is reported only for `http(s)` pages (Chrome says 200 for `data:`) and not for same-document navigations. A 4xx/5xx is a warning: `the server answered HTTP 404 Not Found for <url>`.
- 3.3: `details.reason` values: `dns`, `connection_refused`, `connection_failed`, `timed_out`, `offline`, `certificate`, `tls`, `too_many_redirects`, `unsafe_port`, `blocked`, `file_not_found`, `invalid_url`, `bad_response`, `download`, `aborted`, `other`; `details.error` keeps Chrome's `net::ERR_*`. A reload or history step has no `errorText`: when it lands on Chrome's error page (`unreachableUrl`), the code is read from the page's `loadTimeDataRaw.errorCode` (else `net::ERR_FAILED`). A failed `goto` waits up to 2 s for the error page to commit, so the next command does not race it. The error page's frame URL is the URL that failed, as the address bar shows it.
- 3.3: Timeouts (exit 3) say what was reached: `<url> did not answer within 30s` when `Page.navigate` got no answer, else `<url> did not reach the load event within 30s` with notes `reached: commit, domcontentloaded` (named as `--wait` values) and the page's URL and title; the hint is the same command with `--wait domcontentloaded` (or `--wait load` for `networkidle`) or a longer `--timeout`.
- 3.3: Relative URLs are those starting with `/`, `./`, `../`, `?` or `#` (`a.html` is a host, like `example.com`). They resolve in the daemon, with standard URL rules (`/login` against `https://x/app/` is `https://x/login`), against `base_url` when set, else the main frame's URL; with neither (a blank tab) it is `BadArgs` with a full-URL hint. `file://` URLs work; a bare file path is not guessed, since it would clash with relative paths.
- 3.3: `goto` starts the session's browser like `cdp` and `tab new`; `back`, `forward` and `reload` need a running session (`DaemonUnavailable`). `goto -` reads the URL from stdin. At either end of the history `back`/`forward` exit 0 with `navigated: false` and say so. Output: `→ 200 <final url> "Title" (412ms)`, with `(same document, …)` or `(from the back/forward cache, …)` when that is what happened; `-q` prints the final URL. `open <url>` and `tab new <url>` use the same navigation, so they report HTTP errors as warnings too, and `open --json`'s `page` gained `status`.
- 3.3: `--ignore-https-errors` is an `open` flag and a session setting like `--follow-popups` (kept across a daemon upgrade, never forces `--restart`): `Security.setIgnoreCertificateErrors` on the browser, sent at launch and at once to a running one. The certificate error's hint is `chrome-cdp open --ignore-https-errors`. The fixture server got an HTTPS origin (`Fixtures::https`, a committed self-signed certificate for `127.0.0.1`) to test it, plus `?status=<code>`, `/redirect?to=&n=` and `nav/` pages.
- 3.3: Same-document navigations end a history wait (`navigatedWithinDocument`) and are reported as `same_document`; the frame tree already follows their URLs. Plan 04's ref invalidation should hook the same event in `tabs::frame_event`.
- 3.3 (the 3.2 follow-up): a document restored from the back/forward cache (`BackForwardCacheRestore`) did lose its same-site iframes: Chrome re-announces only some frames. `FrameTree` now keeps the subframes of the last 8 documents by loader id and puts them back on a restore; `frames/nested.html` → other page → `back` lists all five frames and every frame answers in both worlds.
