# Plan 03 — Pages, navigation & waiting

Status: DONE
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

- [x] `goto` + default settle gives a correct snapshot on the SPA fixture with
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
- 3.4: `wait_until(condition, timeout)` in `engine::wait` has no `poll` argument: the conditions only the page knows (element state, text, `--fn`, the load state) are one `Runtime.callFunctionOn` of `wait/probe.js` that checks on every animation frame, with a 100 ms timer as well because hidden tabs run no frames; it starts again in the next document when a navigation destroys its context. Network idle and `--request`/`--response` are event-driven from a per-tab tracker (`engine::network`), and `--url` from the frame tree, through one `Tabs::watch` loop that re-checks on every change and at the instant a check asks for.
- 3.4: Every page and OOPIF session now gets `Network.enable` during setup (plan 06 can take its buffers from the same events). Idle ignores the resource types `EventSource`, `Ping` (beacons) and `CSPViolationReport` (WebSockets are not requests in CDP) and requests open over 10 s. Chrome does not always end the requests of a document that is replaced, so a main-frame `frameNavigated` drops the session's other in-flight requests (a subframe's, those of that frame).
- 3.4: The 500 ms of quiet count from the later of the last request activity and the wait's start (a navigation's start for its own settle), so `wait --idle` right after an action sees requests the action starts a moment later, at the cost of taking at least 500 ms.
- 3.4: The settle is network idle, then a MutationObserver in the isolated world of the main frame (child lists, attributes, text) until 300 ms pass without a change or 3 s in all, then network idle again (a DOM change may have started a request). A DOM that never stops changing (a clock) still settles after 3 s: only the network part can fail.
- 3.4: The navigation default is a new `--wait settle` (the load event, then the settle), for `goto`, `back`, `forward`, `reload`, `open <url>` and `tab new <url>`; `--wait load` skips the settle. A navigation's settle stops after 5 s with a warning naming the requests in flight rather than failing the navigation: the page did load. `wait --settle` is the strict form (a timeout error). `--wait networkidle` is now `DOMContentLoaded` plus the tracker instead of Chrome's `networkIdle` (which never fires on pages with beacons or long-polls); its timeout lists the requests in flight, and `reached` no longer names `networkidle`.
- 3.4: `wait <locator>` takes CSS (`css=…`, or a bare `#`, `.`, `[` selector) and XPath (`xpath=…`, or a bare `//…`) only; refs, `text=`, `role=` and the rest are `BadLocator` errors (exit 2) that say what works. `engine::locator` is where plan 04 adds them and `frame=` scoping; waits look in the main frame. With several matches, `visible`/`attached`/`enabled` need one match and `hidden`/`detached` all of them. Visible is Playwright's: a non-empty box and not `visibility: hidden`; enabled is not `:disabled` nor inside `aria-disabled="true"`. A selector the page rejects fails at once as `BadLocator`.
- 3.4: `--text` looks in `innerText` (so only rendered text, not iframes) of the body or the first `--in` match, with runs of whitespace collapsed on both sides; it is case-sensitive, so a copied string matches exactly what the page shows.
- 3.4: URL patterns (`--url`, `--request`, `--response`): `/regex/flags` (Rust `regex` syntax, flags `i`, `m`, `s`), a glob with `*` (no `/`) or `**` matching the whole URL, else a part of the URL (`--url /dashboard`). The `regex` crate is new (MIT/Apache-2.0; its automata were already in the tree through tracing-subscriber).
- 3.4: `--request`/`--response` match what arrived since the *previous* page command on the tab began, not only during the wait: a `click` and then `wait --response /api/save` must not miss a fast response. `Engine::page` marks each command's start in the tab's 256-entry request log. On timeout, a matching response with another status is named (`got: response 201 POST …`).
- 3.4: `--fn` runs in the page's main world (its globals) through `callFunctionOn`, so a page's CSP cannot block it; a promise is awaited, a throw counts as falsy (the timeout names the last error), and a syntax error fails at once (`EvalError`, exit 1). Its value is returned as JSON (else as a string) in `data.value` and `-q`.
- 3.4: `wait <ms>` accepts `500` or `2s`, is not bounded by `--timeout`, and warns toward a condition. Timeout errors say what was seen (element counts, text found or not, the URL, the last `--fn` value or error, requests in flight, `document.readyState`) plus the page's URL and title; the hint is the same `wait` with a doubled `--timeout` and the likely reason. Console errors are not counted yet: plan 06 owns console capture.
- 3.4: `--timeout` has no clap default any more; `wait` defaults to 10 s and every other command to 30 s (`cli::WAIT_TIMEOUT`, which plan 05's actions should use). The flag, `CHROME_CDP_TIMEOUT` and the config key still win. clap ignored `requires` on `wait`'s flags next to its required condition group, so `--state`, `--in` and `--status` without their partner are checked by hand (`BadArgs`).
- 3.4: The fixture server got `/ws`, a WebSocket echo, and `testdata/site/wait/` pages (beacons, WebSocket, lazy content, an SPA rendering from data fetched after load, a slow request, a DOM that never stops changing). The SPA acceptance box needs plan 04's snapshot: `goto_settles_by_default` checks the rendered DOM instead (5 runs by default, `CHROME_CDP_SETTLE_RUNS=50` passed 50 in a row).
- 3.5: `--dialogs auto|accept|dismiss|manual` is an `open` flag and a session setting like `--follow-popups` (`auto`, the default, accepts `alert` and `beforeunload` and leaves `confirm`/`prompt` open; `accept` gives a prompt its default). The daemon now also records session settings given to an already running browser in `meta.json`, so `--dialogs`, `--follow-popups`, `--ignore-https-errors` and `--http-credentials` survive a daemon upgrade in that case too.
- 3.5: "The pending action returns right away" is a `DialogOpen` error (exit 5): `dialog open in tab t1: confirm "Delete item?"`, hint `chrome-cdp dialog accept, or chrome-cdp dialog dismiss` (`'<text>'` for a prompt, `--tab` when it is not the active tab), `details.dialog`. Page commands race their work against the tab getting a dialog that opened after they began (or a crash), so they return at once instead of timing out. A dialog already open: `wait` conditions that run page script fail at once; navigations go ahead (Chrome dismisses it, and the output says so); a raw `cdp` call is sent as is (it may be `Page.handleJavaScriptDialog`) and warns if the tab is still blocked afterwards.
- 3.5: `dialog accept [text]` / `dialog dismiss` answer the tab's dialog on the session that reported it (an OOPIF's own). They take no tab lock (a raw call may still wait on the page) and do not wait for what the answer causes; `wait` does. Text is only for a prompt (`BadArgs` otherwise). With no dialog in the tab, `NotFound` names the tabs that have one. `tabs` shows open dialogs and crashed pages, with the command that unblocks each, under the table.
- 3.5 (follow-up): `dialog accept|dismiss` with no dialog open in the tab waits for one up to `--timeout` (a prompt a page opens right after its confirm closes raced the next batch step), then fails `NotFound` with a `waited` note as `click` does; if another tab has one open it fails at once, since that is most likely the dialog meant.
- 3.5: Dialogs answered without a `dialog` command are reported by the command that ran meanwhile, as `events` (`dialog`, with `by: policy|other`) and a warning (`dialog: alert "Saved" (accepted)`), like opened tabs; downloads likewise (`download` events). Plan 05's post-action report should move these lines to stdout (`Engine::report_events`).
- 3.5: When the policy dismisses a `beforeunload` during a navigation, Chrome cancels the navigation without an error (the `goto` then ran into its timeout or a bare `ERR_ABORTED`), so the navigation now fails at once: `NavigationFailed`, `details.reason: beforeunload`, hint `open --dialogs auto`. Testing also showed a second navigation started before the first commits triggers `beforeunload` again.
- 3.5: Downloads use `Browser.setDownloadBehavior {allowAndName, eventsEnabled}` into `<session dir>/downloads` (0700, made at launch, kept after `close`), and a completed file is renamed from its guid to the server's name made safe and unique (`report (1).txt`), so a half-written file never has the final name. A connected browser gets the same behaviour and its default back on `close`. `goto` to a download (the goto-download follow-up) now succeeds: it waits for the file, `navigated: false`, `wait: "download"`, `data.download`, text `downloaded: report.txt (27B) → <path>`, `-q` the path; the old `NavigationFailed`/`download` error remains only when the browser saves nothing. `wait --download` waits for a download that began since the previous command began (like `--response`) to complete or be canceled; `downloads` lists the session's last 256.
- 3.5: A crashed page (`Inspector.targetCrashed` on the tab's own session; an OOPIF's crash leaves the page up) is a new error kind, `PageCrashed` (exit 4), rather than `BrowserCrashed`, whose hint (`open --restart`) would be wrong: its hint is `chrome-cdp reload`. Commands running when it crashes return at once; `Page.reload` and `Page.navigate` bring the page back on the same session, with its setup (init scripts, isolated world, events) intact. `TabInfo.crashed` marks it.
- 3.5: HTTP auth needs `Fetch` interception: Chrome rejects `handleAuthRequests` with empty `patterns` and sends `Fetch.authRequired` only for intercepted requests, so with `--http-credentials` every page and OOPIF session intercepts `*` and continues each paused request at once (without credentials nothing is intercepted). A second challenge for the same request is cancelled, so wrong credentials end in `net::ERR_INVALID_AUTH_CREDENTIALS`, now `NavigationFailed` with `details.reason: auth` and hint `open --http-credentials USER:PASSWORD`. The flag takes `-` for stdin and `CHROME_CDP_HTTP_CREDENTIALS`; the credentials are kept in the 0600 `meta.json` for upgrades, and `Debug` output hides the password. Service and shared workers' requests are not intercepted.
- 3.5: The fixture server got `?download=<name>` (served as an attachment) and a basic-auth area `/auth/` (`fixtures::AUTH_USER`/`AUTH_PASSWORD`), plus `dialogs/on-load.html?kind=&message=&default=`, `dialogs/beforeunload.html` and `downloads/`. The golden normaliser now masks ` (…)` as a time only when it ends in `s`.
- 3.5 (a 3.4 bug found while testing): a cross-site iframe's document request is sent on its parent's session but ends on the iframe's own, so the parent's tracker kept it in flight and every navigation to a page with an OOPIF waited out the 5 s settle with a warning. A frame that commits a document now ends the requests other sessions sent for it.
- Close-out: every item of the Tests section was checked against the tests. The timeout cases missing for `--domcontentloaded` (a new `wait/slow-script.html` holds it up with a parser-blocking script), `--state enabled|detached|attached`, `--request` and the settle's in-flight note were added to `crates/engine/tests/wait.rs`; `wait <ms>` has no timeout by design. The reload test's one-page server now serves until stopped, since a connection Chrome opened without a request made the page load fail now and then. `goto_settles_by_default` passed 50 runs in a row again (`CHROME_CDP_SETTLE_RUNS=50`); the SPA box stays open until plan 04's `snapshot` can be checked the same way.
- (from plan 04, 4.1/4.3) The SPA box is ticked: `goto_settles_by_default` now also checks that the `snapshot` right after `goto` ends with the heading and the three inbox items, and passed 50 runs in a row (`CHROME_CDP_SETTLE_RUNS=50`).
- (follow-up) The active tab stays in front: Chrome shows a popup in front of its opener, which then reports `visibilityState` "hidden" (no animation frames, throttled timers, geolocation never answers). A popup chrome-cdp does not follow brings the active tab of its context back with `Page.bringToFront` before it counts as ready, and a command's report waits (up to the same 500 ms) for popups still being set up, so the next command never sees the opener hidden. Not `Emulation.setFocusEmulationEnabled` on every page (Playwright's way): it would also make every page focused, which plans 05 and 07 chose not to do outside `clipboard`.
- (follow-up) `tab <id>`, in a context too, and `tab close` bring the (new) active tab to the front with `Page.bringToFront`: `Target.activateTarget` leaves a tab of a `context new` context hidden in headless Chrome, and after a close Chrome shows the closed tab's neighbour, not the tab chrome-cdp falls back to.
