# Plan 13 — Every frame, every worker

Status: IN PROGRESS (13.1-13.3 done)
Depends on: v1 (plans 01-11). Track 1 of [plan 12](12-after-v1.md).

## Goal

A feature that works in the main frame but not in an iframe or a worker
is a bug, not a gap (plan 12 §2). Today many features are wired to the
page's own session, or to "page and iframe sessions", and miss the rest:

- dedicated workers get only their console;
- shared and service workers get nothing (they attach at the browser
  level with no tab);
- several readers (`text`, `markdown`, `wait --text`, the focus line of
  the action report, `expect --a11y`, coverage, `cdp --listen`) look at
  one session or one document.

Seventeen backlog notes are this one bug. Instead of seventeen patches,
this plan gives the engine one model of the targets a tab (or a context)
owns, and makes features declare which kinds of target they apply to.
The engine applies each feature to every matching target: those that
exist, and those that attach later, before they run a script.

When this plan is done, a new feature gets frames and workers by
declaring them, and a table test fails if it forgets.

## Background: how targets are handled today

Read these before starting any milestone.

- `crates/engine/src/tabs/mod.rs` is the target and tab manager (plan
  03, 3.1). The browser session auto-attaches to every target with
  `waitForDebuggerOnStart` (`auto_attach()`, `watch_browser`), and so
  does each page and iframe session. `attached()` handles
  `Target.attachedToTarget`:
  - a page or iframe (`type` `page` / `iframe`) becomes a `Host` in
    `State::hosts: HashMap<String, Host>` (keyed by session id), with
    `watch_host` and `fetch::watch_fetch` tasks;
  - a `worker` announced by a host session is a tab's dedicated worker:
    `watch_worker` feeds its console to the tab;
  - anything else (`shared_worker`, `service_worker`, others) is
    `Kind::Other`: it is resumed and otherwise ignored.
- `setup()` builds a list of steps for a new target by `Kind` (`Host`,
  `Worker`, `Other`), sends them all, then `Runtime.runIfWaitingForDebugger`.
  Features add their steps through `inject::setup_steps`,
  `fetch::setup_steps` and `emulation::setup_steps` (in `tabs/`).
- When a setting changes later, each feature has its own `apply()` that
  loops over `State::hosts` (`fetch::apply`, `emulation::apply`,
  `inject.rs`). Each keeps its own per-host state on `Host`
  (`interception`, `clock`, `injected`).
- A host's `scope` is its browser context's settings key (`None` for the
  session's default context). Contexts: `crates/engine/src/context.rs`,
  `Settings::contexts`.
- Frames: `crates/engine/src/frames/` (`FrameTree`, `FrameInfo`,
  `Engine::frames` in `frames/commands.rs`). OOPIFs have their own
  session; `FrameInfo::oopif` says so.
- Fixtures: `crates/fixtures` serves `testdata/site` on two origins,
  `http://127.0.0.1:<port>` (`Fixtures::a`) and
  `http://localhost:<port>` (`Fixtures::b`). They are different sites,
  so an iframe of one in the other is an OOPIF. Both count as secure
  contexts, so service workers can register on either.
- Existing worker fixtures: `testdata/site/workers/worker.html`,
  `worker.js`; tests in `crates/engine/tests/tabs.rs`.

## Design

### Targets

Replace `Host` and `State::hosts` with one map of every attached target,
keyed by session id:

```rust
struct Target {
    session: Session,
    kind: TargetKind,        // Page, Iframe, DedicatedWorker, SharedWorker, ServiceWorker, Other
    target_id: String,
    url: String,             // kept current from targetInfoChanged
    owner: Owner,
    scope: Option<String>,   // the context's settings key, as Host::scope
    parent: Option<String>,  // the session that announced it (None: the browser)
    // per-feature state that lives on Host today: clock, interception, injected, …
}

enum Owner {
    Tab(u32),                                  // pages, iframes, dedicated workers
    Origin { origin: String },                 // shared and service workers
    None,                                      // prerenders, others
}
```

- A dedicated worker's owner is the tab of the session that announced it
  (a worker started in an OOPIF belongs to the OOPIF's tab too).
- Shared and service workers belong to an origin in a scope. "The tabs
  of a worker" are the tabs of the same scope that have a frame on that
  origin, computed when needed (a helper `Shared::tabs_of(&Target)`).
- `Kind` in `setup()` goes away in favour of `TargetKind`.
- Keep the name `Host` only if it makes the diff smaller; the goal is one
  map that everything iterates.

### Features

A feature says what it needs per target kind, once:

```rust
trait Feature: Send + Sync {
    fn name(&self) -> &'static str;
    /// The kinds this feature applies to.
    fn kinds(&self) -> KindSet;
    /// Steps for a new, paused target of a matching kind (sent before resume).
    fn setup_steps(&self, shared: &Arc<Shared>, target: &Target, steps: &mut Vec<(&str, Step)>);
    /// Bring every matching target (of `scope`, or all) in line after a
    /// setting changed.
    async fn apply(&self, shared: &Shared, scope: Option<&str>, timeout: Duration) -> Vec<String>;
}
```

(An `async fn` in a trait object needs a boxed future; use whatever
shape fits the code. A plain enum with a `match` per feature is also
fine if it reads better. What matters: one registry, iterated by
`setup()` and by every `apply`.)

- The registry is a fixed list built at engine start: base domains
  (Runtime, Log, Network, Page, lifecycle, the isolated world), console,
  network log, fetch (routes, mocks, auth), inject (init scripts,
  expose), emulation, clock, coverage (while on), and the mock shim.
- `setup()` asks each feature whose `kinds()` include the target's kind
  for its steps, then resumes.
- A feature turned on later (`coverage start`, a first `route`, `time
  freeze`) calls its own `apply`, which uses the same kind filter.
- Workers have no `Page` domain. Scripts that pages get through
  `Page.addScriptToEvaluateOnNewDocument` go to a paused worker as
  `Runtime.evaluate` before `runIfWaitingForDebugger`, which runs before
  the worker's own script. A worker that is already running gets the
  script late; say so in the command's output only if it matters
  (the frozen clock: `time freeze` reports `2 workers already running`).

### Where worker events go

- Dedicated worker: its tab, as now, with the worker's URL on each
  entry.
- Shared and service worker: recorded, at event time, into each tab of
  the worker (see Owner above). Entries carry a `worker` field
  (`{kind, url}`) so text output can show `[service worker sw.js]`.
  `expect --no-errors --all-tabs` counts each worker entry once
  (dedupe by worker target id and the entry's own sequence). Events of a
  worker with no tab are dropped and counted; `status` shows the count.
- `--json` only gains fields (`worker`, optional). No schema loses or
  renames anything (docs/output.md).

### The matrix fixture and the table test

`testdata/site/targets/matrix.html` on origin A has:

- a same-site iframe (origin A, `targets/frame.html`);
- a cross-site iframe (origin B, `targets/frame.html`), an OOPIF;
- a dedicated worker, a shared worker and a service worker
  (`targets/dedicated.js`, `shared.js`, `sw.js`, scope `/targets/`),
  each also started from the cross-site iframe where that makes sense.

Every frame and worker can, on a `postMessage` or a URL flag: fetch
`/targets/api?from=<who>`, log to the console, throw, report
`navigator.userAgent`, `navigator.language`, the time zone, `Date.now()`
and `Math.random()`. Pages expose the results in the DOM so tests read
them with `eval` or `text`.

`crates/engine/tests/targets.rs` holds a table: rows are features,
columns are target kinds (main frame, same-site iframe, OOPIF,
dedicated, shared, service worker). Each cell is a check or an explicit
`n/a` with a reason (for example `emulate viewport` in a worker). A row
added by one milestone starts with every cell it does not cover yet
marked `todo`; the plan is done when no `todo` is left.

## Milestone 13.1 — Target graph

- Build `Target`, `Owner`, `TargetKind` and the feature registry above
  in `crates/engine/src/tabs/`. Move every existing setup step and every
  `apply` onto it. **Behaviour does not change** in this milestone: each
  feature declares exactly the kinds it covers today (hosts; console for
  dedicated workers).
- Track shared and service workers as targets with `Owner::Origin`, and
  keep their `url` current (`Target.targetInfoChanged`).
- Detach and target-destroyed paths remove targets from the one map
  (today `detached()` removes hosts; `State::remove` drops a tab's).
- Build the matrix fixture and the table-test harness with the rows for
  what already works, and `todo` cells for the rest.
- Backlog: `tabs-other-workers`. Test that a shared and a service
  worker are resumed and answer, so a paused worker can never hang a
  page. Its note also says `--http-credentials` only intercepts page and
  iframe sessions: leave that for 13.2 (it is fetch).
- Tests: the existing engine and CLI suites pass unchanged; a unit test
  for `tabs_of` (origin and scope matching); the matrix page loads with
  every target attached and resumed.

## Milestone 13.2 — Network, routes and mocks in every target

- `network`: `Network.enable` on dedicated, shared and service workers;
  their requests go into the tab's `NetLog` (see "Where worker events
  go"), marked with the worker. `network` text shows the marker; `--json`
  gains `worker`. Backlog: `network-worker-requests`.
- Routes and mocks: `Fetch.enable` on worker sessions, events through
  `fetch::watch_fetch` with the owning tab (for shared and service
  workers, the target itself, with a scope but no tab; routes and mock
  sets are per scope, so this works). HTTP credentials come with it.
  Keep `Network.setBypassServiceWorker` while routes exist, so page
  requests still skip the service worker and reach the router; the
  service worker's own fetches are now routed too. Unmatched-request
  checks (plan 11.5) see worker requests. Backlog:
  `route-worker-requests`, and the credentials part of
  `tabs-other-workers`.
- Mocked WebSockets and EventSource in workers: the shim
  (`crates/engine/src/route/shim.js`, `tabs/mock_shim.rs`) is an init
  script today; run it in workers through the worker script path of the
  design. Add binary messages: base64 over the binding, and a
  `send_base64` action (or the name that fits the mock format) for
  `[[websocket]]` mocks; extend `docs/e2e/mocks.schema.json` (additions
  only). Backlog: `mock-shim-workers-binary`.
- Tests: table rows for `network`, `route`, `mock use` (HTTP), mocked
  WebSocket, `expect request`, the unmatched check, HTTP auth.

## Milestone 13.3 — Console, errors, emulation and determinism in every target

- Console and errors: `Runtime.enable` and `Log.enable` on shared and
  service workers, entries recorded as in the design, text and `--json`
  as above. Backlog: `console-shared-service-workers`.
- Emulation: user agent, locale, time zone and the frozen clock in every
  worker. Check first which `Emulation.*` commands Chrome 154 accepts on
  worker sessions (the live `/json/protocol` and a quick call); use them
  where accepted, and fall back to `Network.setUserAgentOverride` (it
  also sets `Accept-Language`) and a `Runtime.evaluate` override
  otherwise. Record what works in the decisions log. `emulate`'s own
  check (that the locale and time zone took) must look at a worker too.
  Backlog: `emulate-workers`.
- Determinism (`crates/chrome-cdp/src/test/determinism.js`, plan 11.6):
  the seeded `Math.random`/`crypto` and the frozen clock reach workers
  through the init-script path; the no-animations sheet is adopted in
  open shadow roots too (wrap `attachShadow` and cover roots that exist
  already). Backlog: `determinism-workers-shadow-dom`.
- Permissions per context: `permissions` and `geo`'s grant send the
  context's `browserContextId`, and `Emulator::permissions` is keyed by
  scope like `Emulator::scoped`. Backlog: `context-permissions`.
- Tests: table rows for `console`, `errors`, `expect --no-errors`
  (with `--all-tabs` counting a shared worker's error once), `ua`,
  `locale`, `tz`, `time freeze`, `time advance`, the determinism
  script; a context test for `permissions grant` in two contexts.

## Milestone 13.4 — Seeing the targets

- A `frames` command: the tab's frame tree, OOPIFs included, plus its
  workers, as an indented tree (`frame main …`, `iframe "pay"
  https://… (out of process)`, `worker dedicated …`, `service worker …`),
  and `--json` with a schema in `protocol::schema` (the `FrameInfo` list
  plus workers). Frame errors (a bad `--frame`, `frame=` not found) point
  their hint at `frames`. Help, `docs/commands/frames.md`, golden tests,
  and a line in `chrome-cdp skill`. Backlog: `frames-listing-command`.
- Sibling order: sort siblings by their owner element's DOM position
  (`DOM.getFrameOwner`, as `snapshot` does), not arrival order, so
  `frames` and frame errors are stable after `connect`. Backlog:
  `frame-order-on-connect`; restore the order check in the engine frames
  test.
- `cdp --listen` on every session of the tab: `--all-sessions` (or the
  default, if that reads better; decide and record) subscribes to each
  frame and worker session of the tab and prefixes each line with the
  frame's or worker's name. Backlog: `cdp-listen-child-sessions`.
- Coverage across OOPIFs: `coverage start` turns on `Profiler` and CSS
  rule tracking on every OOPIF session of the tab, and on those that
  attach while it runs (it becomes a registry feature with a live
  on/off); `coverage stop` sums them and marks each file's frame.
  `crates/engine/src/observe/coverage.rs`. Backlog:
  `coverage-cross-site-iframes`.
- Tests: `frames` on the matrix page (text and JSON, stable order across
  10 `connect` runs); `--listen 'Network.requestWillBeSent'` sees the
  OOPIF's and a worker's requests; coverage counts the OOPIF's script.

## Milestone 13.5 — Reading and acting across frames

- `text` and `markdown` without a locator include each visible iframe's
  content where its `<iframe>` sits, labelled with the frame's name (or
  URL), read through the frame's own session (as `box` does for
  offsets). `text frame=<name>` stays as is. Backlog:
  `read-page-text-iframes`.
- `wait --text` / `--text-gone` without `--in` check every frame of the
  tab (one probe per frame context; done when any has the text, or when
  none has it for `--text-gone`), and the timeout error lists the frames
  read. Backlog: `wait-text-in-iframes`.
- The action report's `! focus moved to …` follows focus into iframes:
  mark focus in the frame that holds it (follow `activeElement` into
  same-process iframes; ask OOPIF sessions). `injected/report.ts`,
  `crates/engine/src/input/report.rs`. Backlog:
  `report-focus-in-iframes`.
- `expect --a11y` runs its rules (`injected/a11y.ts`) in each frame's
  isolated world, as `snapshot` walks frames, and names the frame in
  each finding. Backlog: `expect-a11y-iframes`.
- `drag` inside a cross-site iframe: intercept drags on the iframe's
  session too (`Input.setInterceptDrags`, `input/drag.rs`). `scroll
  --by/--to` without a locator picks a point whose scroll container is
  the page, not a nested scroller in the viewport's middle. Backlog:
  `drag-scroll-edge-cases`.
- `history --as-scenario`: an element in a frame gets a `frame=`
  locator for its `<iframe>` (the same stable candidates, computed in the
  parent frame) as a prefix, e.g.
  `click 'frame=#pay >> role=button[name="Pay"]'`, instead of a ref and a
  comment. `injected/stable.ts`, `crates/daemon/src/history.rs`.
  Backlog: `history-frame-locators`.
- Tests: fixture pages for each case (text inside a same-site and a
  cross-site iframe; a form in an OOPIF that takes focus; an a11y
  violation in an iframe; a drag list in an OOPIF; a fixed sidebar that
  scrolls in the viewport's middle; a history export that replays).

## Tests

- `crates/engine/tests/targets.rs`: the table test (every row, every
  target kind, no `todo` left at close-out).
- The matrix fixture under `testdata/site/targets/`.
- The CLI golden tests for `frames` and every output that gained a
  worker marker.
- The existing suites pass unchanged after 13.1.

## Acceptance criteria

- [ ] One map of targets; every feature registers its target kinds, and
      no feature loops over sessions on its own.
- [x] Shared and service workers are resumed and answer (no page can
      hang on a paused worker).
- [x] Requests from every kind of worker are logged, routed and mocked;
      an unmatched worker request fails a test like a page's does
      (a shared worker's only as far as Chrome allows: see the decisions
      log, 13.2).
- [x] Console output and errors of every worker reach the right tabs;
      `--all-tabs` counts each once.
- [x] Emulation, the frozen clock and the determinism script reach
      workers; the no-animations sheet reaches open shadow roots.
- [x] `permissions` applies per context.
- [ ] `frames` lists frames (stable order) and workers; frame errors
      point at it.
- [ ] `cdp --listen`, coverage, `text`, `markdown`, `wait --text`, the
      focus report, `expect --a11y`, `drag` and `history --as-scenario`
      cover iframes, OOPIFs included.
- [x] The table test has no `todo` cell.
- [ ] Every `--json` change only adds (`cargo xtask release check`
      passes).

## Decisions log

- 13.1: The registry is an enum (`tabs/features.rs`, `Feature`) with a
  `match` per feature, not a trait: each `apply` keeps its own arguments
  (`aspects`, `force`, `now`), and finds its targets through
  `State::applies(feature)`, the same kind filter `setup()` uses.
- 13.1: The "base domains" are four features, `base` (auto-attach,
  `Page`, lifecycle), `runtime` (`Runtime`, `Log`: contexts and console),
  `network` and `world` (frame tree, isolated world, perf observer), so
  `runtime` alone reaches dedicated workers and the steps keep their
  order. Console, network log and the mock shim are not features of
  their own: their events come from `runtime`/`network`, and the shim is
  an `inject` item.
- 13.1: Coverage is not in the registry yet: it is turned on by `coverage
  start` on one tab's session only; 13.4 makes it a registry feature with
  a live on/off, as that milestone says.
- 13.1: `Target`'s Chrome id is `id` (clippy's `struct_field_names`
  rejects `target_id` on `Target`). `tabs_of` is on `State` (callers
  hold the lock), not on `Shared`.
- 13.1: A detached target takes the targets it announced with it
  (`State::forget_target`): their own detach events came through its
  session and may never arrive.
- 13.1: A dedicated worker of a prerender (no tab) now gets `runtime`'s
  `Runtime.enable` like any dedicated worker; nothing reads its events.
- 13.1: The table test (`crates/engine/tests/targets.rs`) has an `eval
  --frame` row so `n/a` cells exist from the start; its `todo` cells were
  checked to fail today.
- 13.2: Chrome 154 has no `Fetch` domain in a dedicated worker
  (`'Fetch.enable' wasn't found`, at setup too): its requests are paused
  in the session of the frame that started it, with that frame's tab.
  So `fetch` applies to pages, iframes, shared and service workers, and
  a dedicated worker's requests are routed without it.
- 13.2: Chrome 154 takes `Fetch.enable` in a shared worker only before it
  starts, and the worker's loader is made when its script request starts,
  a few microseconds after `Target.attachedToTarget`, whether or not the
  worker waits for the debugger. A shared worker therefore intercepts
  every request from its start (auth challenges too; requests go on at
  once while no route, mock or credential needs them), with the call
  sent from the attach handler itself; nothing is sent to it later. When
  the daemon is starved at that moment (two browsers starting at once in
  a test), Chrome wins and that worker is never intercepted. Tried and
  dropped: a browser-level `Fetch.enable` sees the worker's requests only
  if it is on, with patterns, before the loader is made; an idle one
  (`patterns: []`) does not take patterns later, and one always on would
  pause every request of every page. The table test's `Partly` cells
  (checked, a miss noted) mark the shared worker's route, mock, unmatched
  and HTTP auth cells.
- 13.2: A service worker is also announced by the page sessions it
  controls; that session is only resumed (kind `Other`), and the
  browser's is used, so its requests are logged once.
- 13.2: A worker's own script is requested on the session of the page
  that starts it and, once the worker has `Network` on, answered on the
  worker's: a worker event for a request its session did not send
  applies to the tab's live request of that id (`Network::worker_event`,
  and the same in `NetLog`), so the page no longer waits on it.
- 13.2: Network entries gain `worker` (`{kind, url}`, `kind` one of
  `dedicated`, `shared`, `service`); the console's `worker` (a string,
  since v1) stays as it is. Text lines end with `[service worker sw.js]`.
  Worker requests count for the tab's network idle and the action
  report's failed requests too; a shared or service worker's go to every
  tab of its scope with a frame on its origin, and `expect request`
  counts one once across tabs.
- 13.2: Workers get the mock shim through `Runtime.addBinding` and
  `Runtime.evaluate` (before a new worker's script, late in a running
  one); `Wanted::in_workers` says which injected items reach workers,
  only the shim for now, so 13.3 decides about init scripts. Shared and
  service workers report `Runtime.bindingCalled` without `Runtime.enable`.
- 13.2: Binary messages: `send_base64` (an action's or a rule's, a
  template too) sends one; a binary message from the page fits only
  rules without `match`, with its base64 as `{{body}}`; `mock ws send
  --base64` pushes one. Also fixed: the shim did not turn back on in a
  document after `mock use`, `mock off`, `mock use`.
- 13.3: What Chrome 154 takes on worker sessions (checked on the matrix
  page, every kind of worker, paused and running):
  `Emulation.setUserAgentOverride` is accepted and changes the worker's
  requests' `User-Agent` and `Accept-Language` and its
  `navigator.language`, but not `navigator.userAgent`, which a script
  (`emulate/agent.js`, `Runtime.evaluate`) overrides (`appVersion` too).
  `Network.setUserAgentOverride` does the same as the `Emulation` one, so
  it is not used. `setTimezoneOverride` works in every worker, running
  ones too. `setLocaleOverride` works, but fails with "Another locale
  override is already in effect" in a worker whose renderer has one (a
  dedicated worker, a service worker in its page's process); the locale
  is the renderer's, so that error is ignored for workers and `tz` and
  `locale` compare each worker's value with the page's and warn about one
  that kept another. `setGeolocationOverride` does not exist there,
  `setCPUThrottlingRate` is for pages only, and
  `Network.emulateNetworkConditions` works in shared and service workers
  only ("Not supported" in a dedicated one, whose requests are its
  frame's and follow the page's). A page's user agent reaches the
  dedicated workers it starts later by itself.
- 13.3: Console entries keep `worker` (the script, a string since v1) and
  gain `worker_kind`; text shows `[dedicated worker w.js]` (was
  `[worker]`) like the network log. `Event::ConsoleError` gains `worker`,
  so the action report and `expect --no-errors` name it. A shared or
  service worker's entry goes to each tab of its origin with the same
  key (the worker's target id and its own event number), which `--all-tabs`
  counts once. Console entries and requests of a shared or service worker
  of no tab are counted in `status` (`worker_events_dropped`, shown only
  when not 0).
- 13.3: Init scripts of the page's world run in workers (`Wanted::in_workers`):
  in a new worker before its script, in a running one only with `--now`
  (as in a document); isolated-world scripts, storage seeds and exposed
  functions do not (a worker has no isolated world; `expose` in workers
  is not planned). So the test runner's determinism script, an init
  script, reaches workers with no change of its own but skipping the
  sheet where there is no `document`.
- 13.3: The no-animations sheet reaches shadow roots by wrapping
  `attachShadow` (open and closed roots made from then on) and walking
  the open roots when the document is parsed (declarative shadow DOM);
  a closed declarative root is out of reach.
- 13.3: The first `time freeze` warns how many workers were already
  running (they get the clock late: timers set there before keep real
  time); a later freeze or `time advance` does not, as those workers have
  the clock by then.
- 13.3: `clipboard`'s own permission grant also goes to the call's
  context, like `permissions` and `geo`.
