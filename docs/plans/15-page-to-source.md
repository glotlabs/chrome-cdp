# Plan 15 — From page to source

Status: IN PROGRESS (15.1-15.6 done)
Depends on: v1 (plans 01-11), mainly 04 (refs), 05 (the action report)
and 06 (console, network, source maps). Uses plan 13's target graph where
it exists. Track 4 of [plan 12](12-after-v1.md).

## Goal

A coding agent that sees a wrong button asks next: **which file do I
edit?** Today it greps the codebase for the button's text. The browser
often knows the answer: the framework's dev build records where each
element came from, Chrome can record the stack that created a node, and
source maps turn bundle positions into source lines. Playwright asks
none of this.

This plan connects what the browser shows to the agent's source files:

- `source <loc>`: an element's file and line, and its component chain.
- `components [loc]`: the component tree, with props and state.
- `debug log <file:line> <expr>`: logpoints, printf debugging without
  editing code.
- `errors --locals`: the local variables where an uncaught error was
  thrown.
- `network show <id>`: why a request was sent, as a source-mapped stack.
- Hot-reload awareness: `wait --hmr`, and report lines that say whether
  the page has the agent's last edit.

Every place this plan prints is a **path on disk, relative to the
project** when chrome-cdp can find the file (`src/cart.ts:42`), and the
URL otherwise. That is what an agent passes to its editor.

## Background: what exists today

Read these before starting any milestone.

- **Source maps** (`crates/engine/src/observe/sourcemaps.rs`, plan 06.1):
  `SourceMaps::resolve` sets `StackFrame::original` (a
  `protocol::schema::SourcePosition`: `source` URL, 1-based `line`,
  `column`, optional `name`) for frames of scripts with a map. Scripts
  and maps are loaded through Chrome (`Network.loadNetworkResource`),
  inline `data:` maps included, cached per script URL (256 entries).
  It is on for `localhost` pages (`is_local`) and with `--sourcemaps`.
  It only maps generated → original; nothing maps the other way yet.
- **Rendering places**: `frame_line` and `shown_url` in
  `crates/engine/src/observe/commands.rs` print `fn (/src/app.ts:12:5)`,
  the URL path when it is on the page's origin.
- **Initiators**: `crates/engine/src/observe/netlog.rs` keeps only the
  innermost frame of a request's `Network.Initiator` (`kind`, `url`,
  `line`, `function`), not the stack, and does not map it.
- **The action report** (`crates/engine/src/input/report.rs`): `lines`
  turns events into `→ …` and `! …` lines, at most `LISTED` console
  errors and failed requests each. Its console error line prints the
  generated place (`app.js:1`); that is backlog note
  `report-sourcemapped-console-errors`.
- **Injected code** runs in chrome-cdp's isolated world
  (`crates/engine/src/injected/`, `crates/engine/src/injected.rs`).
  **An isolated world cannot see the page's JavaScript properties on DOM
  nodes** (`__reactFiber$…`, `__vueParentComponent`, `__svelte_meta`):
  each world has its own wrappers. Framework adapters must therefore run
  in the page's main world (see "Adapters" below). Plan 12's sketch said
  isolated world; that is wrong, and this plan replaces it.
- **CDP**: the vendored protocol (`crates/cdp/protocol/*.json`, Chrome
  154) has everything needed. `DOM.setNodeStackTracesEnabled` and
  `DOM.getNodeStackTraces` (experimental), the whole `Debugger` domain
  (`enable`, `setBreakpoint`, `setBreakpointByUrl` with `condition`,
  `setInstrumentationBreakpoint`, `setPauseOnExceptions`, `paused` with
  `callFrames[].scopeChain`, `resume`, `scriptParsed` with
  `sourceMapURL`), and `Network.Initiator.stack`. `Debugger` is not in
  `crates/cdp/protocol/domains.txt` yet: add it (15.4) and run
  `cargo xtask gen-cdp`.
- **Fixtures**: `examples/e2e-shop` commits its built `dist/` so tests
  need no Node, and `crates/chrome-cdp/tests/e2e_shop.rs` checks a hash
  of the sources (`dist/sources.txt`) so a stale build fails. This plan
  copies that pattern. `testdata/site/apps/` has production builds of
  React 18, Vue and Svelte todo apps (no debug info), which stay as they
  are.

## Design

### The project root and disk paths

A place is shown as a disk path when chrome-cdp finds the file:

- The **project root** is the directory the CLI already finds for
  baselines (`crates/chrome-cdp/src/expect.rs`: the nearest
  `chrome-cdp.toml`, `.chrome-cdp.toml`, `.chrome-cdp/` or `.git`), or
  `source_root` in the project config. The CLI sends it with each request
  that prints places; the daemon runs on the same machine and checks the
  disk. With no root, the current directory.
- A source is turned into a path by one function, `places::to_disk`
  (new, in `engine`), used by every command in this plan and by
  `errors`/`console`:
  - an absolute path (React 18's `_debugSource.fileName`, Vue's
    `__file` in dev) is used as it is;
  - Vite's `/@fs/<abs path>` is that absolute path;
  - a URL (`http://localhost:5173/src/app.ts?t=123`) or a bundler URL
    (`webpack://app/./src/app.ts`) loses its scheme, host and query; then
    path segments are dropped from the left until `<root>/<rest>`
    exists. The longest match wins. A match on the file name alone does
    not count (too many `index.ts`).
  - A path inside the root prints relative to it (`src/app.ts`), one
    outside it prints absolute. No match: the URL as `shown_url` prints
    it today.
- JSON: `SourcePosition` gains an optional `file` (the absolute disk
  path). Additive; `source` keeps the URL.
- `errors` and `console` text shows the disk path when there is one.
  This changes text output only where a file was found; the `--json`
  shape only grows.

### Where an element came from

`source` asks, in order, and stops at the first that answers:

1. **The framework's dev-build debug info**, through an adapter:
   - **React 18**: the host fiber (the node's `__reactFiber$<id>`
     property) and its `_debugOwner` chain. `_debugSource` (`fileName`,
     `lineNumber`, `columnNumber`) is the JSX place, already original.
     Component names from `type.displayName || type.name`, through
     `memo` and `forwardRef` wrappers.
   - **React 19** removed `_debugSource`. Fibers have `_debugStack` (an
     `Error` made where the JSX element was created) and `_debugOwner`.
     Parse the stack's frames in the page, drop React's own frames, send
     the first user frame to the daemon and map it with `SourceMaps`.
   - **Vue 3**: the element's `__vueParentComponent` (an instance) and
     its `parent` chain; `type.__name || type.name` and `type.__file`.
     `__file` names the component's file but not the element's line: the
     element's line comes from step 2 (Vue's template render function is
     source-mapped to the template).
   - **Svelte 5**: the element's `__svelte_meta`: `loc` (`file`,
     `line`, `column`) and, in recent 5.x releases, `parent` entries for
     the components and blocks around it.
   Each adapter checks the version it finds and says "not a dev build"
   when the properties are missing. The exact property names must be
   checked against the versions pinned in the fixture apps (15.1 has a
   test for each); record differences in the decisions log.
2. **The node's creation stack**: `DOM.getNodeStackTraces` for the node,
   mapped through source maps, skipping frames whose original source is
   in `node_modules/`, `.vite/deps/`, or a framework runtime. This works
   for plain JavaScript apps and for Vue templates. It only has stacks
   for nodes created after `DOM.setNodeStackTracesEnabled`, so:
   - node stacks are on by default for `localhost` pages (like source
     maps), set up with the tab before its first script runs;
     `open --node-stacks on|off|localhost` and `node_stacks` in the
     config change that. Recording a stack per created node costs page
     time, which is why it is not on for every site.
   - a node with no stack gets the hint to reload: `the element was
     created before chrome-cdp recorded stacks; chrome-cdp reload, then
     run this again`.
3. **Static HTML**: an element parsed from the document's HTML has no
   stack. `source` says `in the HTML of /index.html` (the frame's URL).

### Adapters

- Adapters are one TypeScript module in the injected bundle's source
  tree (`crates/engine/src/injected/src/frameworks.ts`), built into a
  **separate** script that is not installed in any world. The engine
  runs it per call with `Runtime.callFunctionOn` on the node resolved in
  the **main world** (`DOM.resolveNode` with the main world's
  `executionContextId`). Nothing is left on the page's globals.
- An adapter returns plain JSON: framework and version, a list of
  components (innermost first: `name`, a source as a URL or path, line,
  column), and the element's own place if the framework knows it. Values
  (for `components`) are previewed in the page with fixed limits (below)
  and never returned as objects.
- **Trust**: this data comes from the page's own world, so a page can
  lie. That is fine for a debugging aid on the agent's own app, but the
  output marks it: `via react 19 (the page's debug info)`. Nothing in
  this plan acts on it.

### Output

`source`:

```
$ chrome-cdp source e12
e12 button "Add to cart"
  at src/components/CartItem.tsx:42:7
  in <CartItem>  src/components/CartItem.tsx:12
  in <CartList>  src/components/CartList.tsx:30
  in <App>       src/App.tsx:8
via react 19 (the page's debug info)
```

- `at` is the element's own place; `in` lines are the components around
  it, innermost first, at most `--depth` (default 8; the rest are
  counted: `… 3 more`). No framework: `at` from the node stack and no
  `in` lines, and `via node creation stack`.
- `-q` prints the `at` place alone (`src/components/CartItem.tsx:42:7`),
  for `$EDITOR +42 …`-style use.
- Errors: `NotFound` for the locator as usual; a new `NoSource` kind
  (additive) when nothing answers, with a hint for the likeliest reason
  (not a dev build, node created before stacks were on, no source map).

`snapshot --source` adds the innermost component and its file:line to
each node with a ref, as a suffix: `- button "Add to cart" [e12]
<CartItem src/components/CartItem.tsx:42>`. One `callFunctionOn` per
snapshot takes all ref'd nodes (in batches as arguments), so the cost
does not grow with round trips.

`components`:

```
$ chrome-cdp components --depth 3
<App> src/App.tsx:8
  <Header> src/Header.tsx:4
  <CartList> src/CartList.tsx:30  items=[3 items]
    <CartItem> ×3 src/CartItem.tsx:12

$ chrome-cdp components e12
<CartItem> src/components/CartItem.tsx:12   (holds e12)
  props: item={id: 1, name: "Mug", qty: 2}, onRemove=ƒ
  state: #0 2 (useState), #1 false (useState)
  owners: <CartList> ← <App>
```

- Without a locator: the tree from each framework root, `--depth`
  levels (default 4), with siblings of one component and no differing
  props folded (`×3`). With a locator: the nearest component holding
  the element, its props and state in full (within the limits), and its
  owner chain.
- Value previews: depth 2, strings cut at 80 characters, arrays and
  objects over 5 entries shown as `[12 items]` / `{…8 keys}`, functions
  as `ƒ`. `--max-chars` bounds the whole output as usual.
- State per framework: React function components list hook states by
  index (a hook with a `queue` is `useState`/`useReducer`; the rest are
  left out), class components `state`; Vue `props`, `setupState` (refs
  unwrapped) and `data`; Svelte 5 has no public API for component state,
  so it shows the component tree and `state: not available in Svelte 5`.

### Logpoints

```
$ chrome-cdp debug log src/cart.ts:42 total items.length
logpoint 1 at src/cart.ts:42 (bound in /src/cart.ts)
$ chrome-cdp click e12
clicked e12 button "Add to cart"
! logpoint 1 src/cart.ts:42: total=12, items.length=3
$ chrome-cdp debug list
1 src/cart.ts:42  total, items.length  hits 1
$ chrome-cdp debug clear 1
```

- `debug log <place> <expr>...`: `place` is `path:line[:column]`,
  matched against the original sources of the tab's scripts by path
  suffix (the same rule as `to_disk`, both ways). Each `expr` is one
  JavaScript expression, shown with its text as the label.
- **Binding.** Enable `Debugger` on the tab's sessions (plan 13's graph
  when it exists: pages, iframes, workers). For each script whose
  source map lists the source, find the generated positions of the
  original line (a reverse lookup over the map's tokens, new in
  `sourcemaps.rs`; the first mapped column on that line, or the next
  line with code, reported as the line actually used). Set
  `Debugger.setBreakpoint` by `scriptId` with a condition that logs and
  returns `false`, so the page never stops. A script without a map is
  matched by its own URL path.
- **Scripts loaded later** (a reload, a lazy chunk, a hot update): while
  any logpoint is set, `Debugger.setInstrumentationBreakpoint
  ("beforeScriptWithSourceMapExecution")` pauses each new script before
  it runs; the engine binds pending logpoints and resumes. A logpoint
  that matches no script yet is kept as `pending` and says so.
- **The condition** wraps each expression in its own `try`, and passes
  the values to a CDP binding (`Runtime.addBinding`, a name the page
  cannot guess) as previews made in the page with the limits above. An
  expression that throws shows its error in its place (`total=<ReferenceError: total is not defined>`).
- **Where hits go**: the tab's console log, as entries with source
  `logpoint` (so `console --grep` and `console --follow` see them), the
  action report (`! logpoint …`, at most `LISTED`, then `… and N more
  logpoint hits (console shows them)`), and an additive `logpoint`
  event type in `--json`.
- **Bounds**: at most 20 logpoints per session; each stops after
  `--limit` hits (default 1,000) and says so in `debug list` and once in
  the console.
- **Other pauses**: with `Debugger` on, a `debugger;` statement in the
  page would stop it. The engine resumes any pause it did not ask for
  at once, and logs `paused at a debugger statement (src/x.ts:3);
  resumed` to the console.
- `debug clear [<id>|--all]` removes logpoints and, when none remain and
  nothing else needs it, disables `Debugger` again.

### Locals at uncaught errors

- With locals capture on, `Debugger.setPauseOnExceptions("uncaught")`.
  On a pause with reason `exception` or `promiseRejection`, read the top
  3 frames' `local`, `closure` and `block` scopes (not `global`,
  `script` or `module`) with `Runtime.getProperties`, at most 10
  variables per frame, previewed with the limits above, then resume.
  The whole pause is bounded (200 ms; on timeout, resume and record
  what was read).
- The locals are stored with the console entry of the same exception
  (matched by the exception's description and the top frame's script
  and position; record the rule used).
- Capture is on for `localhost` pages by default, off elsewhere;
  `debug locals on|off` changes it for the session.
- `errors --locals` (and `console --locals`) shows them under each
  frame:

  ```
  #14 error uncaught TypeError: Cannot read properties of undefined (reading 'price')
      at lineTotal (src/cart.ts:18:22)
        item = undefined, qty = 2
      at total (src/cart.ts:25:10)
        items = [3 items], sum = 12
  ```

  An entry without locals says why once: `(locals not captured: the page
  is not on localhost; chrome-cdp debug locals on)`.
- JSON: each `StackFrame` gains an optional `locals: [{name, value}]`.

### Request initiators

- The network log keeps the initiator's stack: up to 8 frames, across
  async parents, with a marker per async boundary (`description`, such
  as `Promise.then` or `setTimeout`). The innermost frame stays as today
  (`Initiator` gains an optional `stack`).
- `network show <id>` maps the stack through source maps and prints it:

  ```
  initiator: script
    fetchCart          src/api.ts:12:10
    -- async Promise.then --
    onClick            src/components/CartItem.tsx:57:5
  ```

- `network` (the list) shows the innermost frame's original place when
  it has one.

### Hot updates

The question after an edit is: does the page have my change yet?

- **Detection**, per tab, from the WebSocket traffic the network log
  already sees (`Network.webSocketCreated`/`webSocketFrameReceived`) and
  the console, so nothing is injected:
  - **Vite**: a socket with the `vite-hmr` subprotocol. Messages
    `{"type":"update","updates":[{"path":…}]}` (an update started),
    `{"type":"full-reload"}`, `{"type":"error","err":{message, loc}}`;
    the client logs `[vite] hot updated: /src/x.ts` when it applied one.
  - **webpack-dev-server**: messages `hash`, `ok`, `still-ok`,
    `errors`, `warnings`; the runtime logs `[HMR] Updated modules:` and
    `[HMR] App is up to date.`.
  - **Next.js** (webpack and Turbopack): the `/_next/webpack-hmr` socket;
    `building`, `built`, `sync` actions and their `errors`. Best effort.
  Check the message shapes against the versions this plan tests with
  and record them in the decisions log; they are not a public API.
- **Outcomes**: `applied` (files), `failed` (message, and a place
  mapped with `to_disk` when the error has one), `full-reload` (then the
  load), and `building` while one is under way.
- `wait --hmr`: waits for the next outcome since the previous command
  began (as `--request` does), within `--timeout`. `applied` and
  `full-reload` (after the load) succeed; `failed` fails with the
  compile error. With no HMR client on the tab it fails at once: `no
  hot-reload client on this page (not a Vite, webpack or Next.js dev
  server?)`.
- **Reports**: an action's report gets `! hot update applied
  (src/cart.ts)`, `! hot update failed: <message> (src/cart.ts:12)` or
  `! full reload (src/main.ts)`. Any command on a tab whose outcome
  arrived since that tab's previous command adds the same line as a
  warning, once: the agent edited a file between commands, and this
  says whether the page caught up. JSON: an additive `hot_update`
  event.

## Fixture apps

Tests must run offline and without Node. As with `examples/e2e-shop`,
the apps are built once and their output is committed.

- **Sources**: `testdata/apps-src/<name>/` for `react18`, `react19`,
  `vue`, `svelte` and `plain` (TypeScript, no framework), each a small
  Vite project with a pinned `package.json` and `package-lock.json`.
  `node_modules/` is git-ignored.
- **One app, five times**: a cart. `App` > `CartList` > `CartItem` ×3,
  each item with "Add" and "Remove" buttons; a `fetchCart()` in
  `src/api.ts` on load and on "Add" (for initiators); a "Checkout"
  button whose handler calls `total()` → `lineTotal(item)` that throws a
  `TypeError` on a missing item (for locals); a module-level function
  called on every click (for logpoints). The same `data-testid`s in
  every app, so tests are one table over the five.
- **Dev builds**: `vite build` configured to keep what a dev server
  gives: development mode (React's development build, Vue's dev build,
  Svelte's `dev: true`), no minification, source maps on (external
  `.map` files, so the map-loading path is the real one). Output goes to
  `testdata/site/apps/dev/<name>/`, served by the fixture server on the
  `localhost` origin (`Fixtures::b`), where source maps and node stacks
  are on by default.
- **Staleness**: each build writes `sources.txt` (the e2e-shop hash
  over the app's sources, lock file and config); a test fails when a
  source changed without a rebuild, and says how to rebuild.
  `cargo xtask apps` runs `npm ci && npm run build` in each app; only
  people changing the apps need Node.
- **Presence test**: one test per app asserts its dev-only debug info
  is there (React 18 `_debugSource`, React 19 `_debugStack`, Vue
  `__file` and `__vueParentComponent`, Svelte `__svelte_meta`), so a
  production build fails loudly instead of making `source` look broken.
- **Hot updates** need a live dev server, which needs Node. So:
  - offline: a fake Vite client and server. The fixture server gains a
    WebSocket at `/hmr/vite` that speaks the `vite-hmr` subprotocol and
    sends what a test posts to `/hmr/send`; `testdata/site/hmr/vite.html`
    has a tiny client that, like Vite's, re-imports the updated module
    and logs `[vite] hot updated: …`. Likewise `/hmr/webpack` and
    `hmr/webpack.html`. These cover detection, outcomes, `wait --hmr` and
    the report lines.
  - online: one test gated on `CHROME_CDP_NODE_TESTS=1` runs the real
    Vite dev server of `testdata/apps-src/react19` on a temporary copy,
    edits a file and checks `wait --hmr` and the report. CI's e2e job
    (which has Node) sets the variable.

## Milestone 15.1 — Fixture apps and node stacks

- The five fixture apps (see "Fixture apps"), built and committed, with
  `cargo xtask apps`, the staleness tests and the presence tests.
- `places::to_disk` and the project root plumbing; `SourcePosition.file`;
  `errors` and `console` print disk paths.
- Node stacks: the session option (`open --node-stacks`, config
  `node_stacks`), `DOM.setNodeStackTracesEnabled` in tab setup before
  the first script, and an engine function that returns a node's mapped
  creation stack (frames in `node_modules/` and framework runtimes
  skipped). No command uses it yet except a test.
- The action report's console error lines use the original place
  (backlog `report-sourcemapped-console-errors`; delete the note).
- Tests: `to_disk` unit tests (absolute, `/@fs/`, URL with query,
  `webpack://`, longest suffix, file-name-only refused, outside root);
  for the `plain` app, a node made in `src/cart.ts` has a creation stack
  that maps to its line; a node made before stacks were on has none; a
  click that logs an error reports it at `src/…:line`.

## Milestone 15.2 — `source <loc>`

- The adapters (React 18, React 19, Vue 3, Svelte 5) as described, run
  in the main world with `Runtime.callFunctionOn`; the fallback to node
  stacks, then static HTML.
- The `source` command: text, `-q`, `--json`, `--depth`, `NoSource`.
- `snapshot --source`.
- Help, `docs/commands/source.md`, golden tests, schema.
- Tests: a table over the five apps and ten chosen elements each (a
  button in `CartItem`, the list in `CartList`, a heading in `App`, …),
  each with its expected file, line and component chain; a production
  build (the existing `testdata/site/apps/react.html`) gives `NoSource`
  with the "not a dev build" hint; a page that sets a fake
  `__svelte_meta` is shown as the page's debug info, nothing more;
  `snapshot --source` on the React 19 app names `CartItem` on its
  buttons.

## Milestone 15.3 — `components [loc]`

- The tree and single-component forms, per framework, with the value
  preview limits and sibling folding.
- Help, docs page, golden tests, schema.
- Tests: the tree of each app at `--depth 3`; `components` on an item's
  button shows its `item` prop and its `qty` state; after a click on
  "Add" the state changed; a 200-item list folds; `--max-chars` bounds
  a deep tree; Svelte says state is not available.

## Milestone 15.4 — Logpoints

- `Debugger` in `domains.txt`, regenerated.
- The reverse source-map lookup in `sourcemaps.rs`.
- `debug log`, `debug list`, `debug clear`; binding, pending logpoints,
  the instrumentation breakpoint for later scripts, the CDP binding for
  hits, console entries, report lines, the `logpoint` event, the limits,
  and resuming pauses chrome-cdp did not ask for.
- Help, docs pages, golden tests, schema.
- Tests (on the `plain` and React 19 apps): a logpoint on a line in
  `src/cart.ts` reports its values after a click; one on a line with no
  code moves to the next line and says so; one set before a reload binds
  again after it; one on a lazily loaded module is pending, then bound;
  an expression that throws shows its error and the others still show;
  `--limit 3` stops after three hits; an ambiguous place lists the
  candidates; a page with a `debugger;` statement keeps running; `debug
  clear --all` disables `Debugger`.

## Milestone 15.5 — Locals and initiators

- Locals capture (`setPauseOnExceptions`, scope reading, the time
  bound, matching to console entries), `debug locals on|off`,
  `errors --locals` and `console --locals`, `StackFrame.locals`.
- Initiator stacks in the network log, mapped in `network show`, and
  the original place in the `network` list.
- Help, docs pages, golden tests, schema.
- Tests: the Checkout button's error shows `item = undefined` in
  `lineTotal` and the list in `total`, at `src/…` lines; an unhandled
  promise rejection has locals too; a non-localhost page has none and
  says why; a page throwing 100 errors a second stays responsive and the
  console stays bounded; `network show` for the "Add" request prints
  `fetchCart` in `src/api.ts` and the click handler across the async
  boundary.

## Milestone 15.6 — Dev-server awareness

- The fake HMR servers and pages in the fixtures.
- Detection for Vite, webpack-dev-server and Next.js; outcomes;
  `wait --hmr`; the report lines and command warnings; the `hot_update`
  event.
- The gated real-Vite test.
- Help, docs, golden tests, schema.
- Tests: for the fake Vite and webpack servers, an update → `applied`
  with the file; an error → `wait --hmr` fails with the message and the
  place on disk; a full reload → succeeds after the load; no client →
  fails at once; an update between two commands → the second command
  warns once; the real-Vite test (gated).

## Milestone 15.7 — Agent benchmark and skill

- Agent benchmark (`bench/agent/tasks/`, plan 10.4): four tasks of the
  form "the X button does nothing / shows the wrong total; fix it", one
  per framework app, each with a planted bug in a component file the
  page's text does not name. The task's app runs on a real dev server:
  add a `[setup] dev` option (the command that starts it, with `{port}`)
  next to `serve`, using the `node_modules` that `cargo xtask apps`
  installed. Record per run how many files the agent read (Read, Grep
  and Glob tool calls in the transcript) next to tokens and time.
  Build and check the tasks (`bench agent --check`); do not run them.
- `chrome-cdp skill`: a short "find the code behind it" section
  (`source`, `debug log`, `errors --locals`, `wait --hmr`), within the
  skill's token budget.
- `docs/debugging-a-dev-server.md`: rewrite §3 ("Find out why") and §5
  ("Fix, reload, check") around these commands.
- `docs/playwright-parity.md`: these have no Playwright row; add them to
  the "beyond Playwright" part of the README's comparison table.

## Tests

Integration tests use Chrome for Testing (`CHROME_PATH`) and the fixture
apps on the `localhost` origin, in `crates/engine/tests/source.rs`,
`components.rs`, `debug.rs` and `hmr.rs`, plus CLI and golden tests in
`crates/chrome-cdp/tests/`. Each milestone lists its cases. The only
test that needs Node is the gated real-Vite test.

## Acceptance criteria

- [x] The five fixture apps are committed as dev builds, a stale build
      fails a test, and each app's dev-only debug info is checked.
- [x] `source` names the right file, line and component chain for ten
      chosen elements in each of the React 18, React 19, Vue and Svelte
      apps, and the right line from node stacks in the plain app.
- [x] `source` says why when it cannot answer (not a dev build, node
      created before stacks, no source map), with the next command.
- [x] `components` shows each app's tree and a component's props and
      state, bounded by `--max-chars`.
- [x] `debug log` at a source line shows the value on the next action,
      in `console` and in the report, survives a reload, and never
      stops the page.
- [x] `errors --locals` shows a local of the throwing function and its
      caller for thrown errors and rejected promises.
- [x] `network show` prints a request's source-mapped initiator stack
      across async boundaries.
- [x] `wait --hmr` and the report lines tell applied, failed and full
      reloads apart for Vite and webpack (fake servers), and for a real
      Vite dev server (gated test).
- [ ] Every place printed by this plan's commands, and by `errors` and
      `console`, is a project-relative disk path when the file exists.
- [ ] `--json` schemas only grew; `cargo xtask release check` passes.
- [ ] The agent benchmark has the four "fix the bug behind this
      element" tasks and records files read. (Running it is deferred:
      benchmarks run after all plans.)

## Decisions log

_(append here)_
- 15.1: The apps share one Vite config, `testdata/apps-src/shared/build.mjs`
  (and `shared/public/cart.json`); each app's `vite.config.js` adds its
  framework plugin. Vite's `mode: "development"` alone does not give a
  dev build (plugin-react still emits `jsx`, not `jsxDEV`): the config
  sets `NODE_ENV=development`, which the plugins read.
- 15.1: Dev builds name files by absolute path (React 18's
  `_debugSource.fileName`, Vue's `__file`), which would commit the
  builder's home directory. The build rewrites the checkout's
  `apps-src` to `/build/apps-src`, as a build made in a container would
  be. So `to_disk` also matches an absolute path that does not exist
  the way it matches a URL (longest end under the root); one that exists
  is still used as it is.
- 15.1: Svelte 5 (5.57.1) names files relative to the app
  (`__svelte_meta.loc.file` = `src/components/CartItem.svelte`), so
  `to_disk` takes a relative path as a rest too. Its `loc` is the
  element's `{file, line, column}`; `parent` is a chain of
  `{type: "component"|"each"|…, file, line, column, componentTag}`
  entries up to the root (`parent: null`), which 15.2 can use for the
  component chain.
- 15.1: React 19 (19.3.0) fibers have no `_debugSource`; `_debugStack` is
  an `Error` whose stack starts `Error: react-stack-top-frame`, then
  `exports.jsxDEV`, then the component (`CartItem`), then
  `react_stack_bottom_frame`: the first frame after `jsxDEV` is the
  JSX's place. `_debugOwner` is the owning component's fiber. React 18
  (18.3.1): `_debugSource` is `{fileName, lineNumber, columnNumber}` of
  the JSX element's `<` (not of its attribute lines).
- 15.1: The maps leave out `sourcesContent` (`sourcemapExcludeSources`):
  chrome-cdp needs the mappings, not the text, and the React builds' maps
  went from 1.6 MB to 0.4 MB. The builds are not minified and keep stable
  file names (`assets/index.js`), so a rebuild's diff is readable, and no
  React `StrictMode` (its doubled effects would double the requests
  15.5's initiator tests count). About 3.2 MB are committed in all.
- 15.1: The staleness and presence tests are `crates/engine/tests/apps.rs`;
  the hash covers the app's directory (less `node_modules`) and
  `shared/`. `cargo xtask apps [NAME…]` runs `npm ci && npm run build`.
- 15.1: The project root travels beside each call's params
  (`protocol::rpc::Request::root`), as `--context` does, not in each
  request type: every command that prints a place (errors, console, the
  action report, `expect --no-errors`) gets it without a new field. The
  CLI sets it once per process (`daemon::client::set_project_root`); the
  daemon runs the call inside `engine::in_root`. `source_root` in a config
  file is relative to that file and must be a directory.
- 15.1: `StackFrame` gains `file` too: a script with no source map that is
  a file under the root (an unbundled dev server) prints as a path.
  Disk lookups run only where source maps are resolved (localhost pages
  or `--sourcemaps`), so a remote site's `/static/app.js` is never
  matched to a local file by accident.
- 15.1: Node stacks need `DOM` on while nodes are made; `DOM.disable`
  stops recording (checked on Chrome 154). `coverage stop` and
  `highlight` therefore leave `DOM` on in a session that records. With
  `--node-stacks localhost`, each page or iframe session's top frame
  turns recording on or off at `Page.frameStartedNavigating`, before the
  new document runs a script; the commands are sent from the event
  handler at once. `open --node-stacks` on a running browser applies to
  every page and iframe (for nodes made from then on). The config key
  `node_stacks` is `open`'s default (and an automatic open's), and
  `CHROME_CDP_NODE_STACKS` its env var, like `--console-buffer`.
- 15.1: Reading a node's stack: `DOM.getDocument` (depth 0), then
  `DOM.requestNode` with the element's object id, then
  `DOM.getNodeStackTraces`. The stack's async parents are followed (up
  to 64 frames), mapped with source maps whatever the page, and frames
  whose original place (else URL) is in `/node_modules/`,
  `/.vite/deps/`, `/@vite/`, `/@react-refresh`, `/webpack/runtime/` or
  `/__vite` are left out. `Engine::node_stack` is public for the tests;
  15.2's `source` builds on it.
- 15.1: The report's console error lines (backlog
  `report-sourcemapped-console-errors`): the error's innermost frame (or
  its place) is source-mapped within 500 ms (maps are cached after the
  first lookup) and the line shows the original place, as a project path
  when the file is there (`(src/cart.ts:21)`), else the source's file
  name. The `console_error` event gains an optional `original`
  (`SourcePosition`). `expect --no-errors` uses the same events, and
  `wait --console` maps the entry it shows (and a timeout's list) too.
- 15.2: Vue's node creation stacks hold only Vue's runtime (a template's
  render function returns vnodes; `patch` makes the elements later), so
  they give no Vue line, unlike the plan's step 2 says. Vue keeps no
  lines either, so the element's line is found in its component's
  `<template>` on disk: the only `<tag` there; else the n-th, when the
  template has as many as the component rendered (no `v-for`/`v-if` in
  between); else the only one whose line holds the element's text; else
  no line (the file alone, with a warning). A component's line is its
  child's tag (`<CartItem`, or `<cart-item`) in its template, the only
  one. Only `.vue` files are read. JSON marks these places `matched`,
  and the text's `via` line says `lines found in the templates`.
- 15.2: A component's `in` line is its file at the line that renders the
  next step towards the element (React's `<CartItem` JSX in
  `CartList.tsx`, Svelte's component entries, Vue's template tag), not
  the line where the component is defined, as the plan's example shows:
  that is what the frameworks' debug info records, and a definition's
  place needs `[[FunctionLocation]]` and the script's URL (`Debugger`).
  So the innermost component's line is the element's own.
- 15.2: React has no version on the page without the DevTools hook: the
  adapter says `18` when fibers have `_debugSource` and `19` when they
  have `_debugStack`. Vue's version is the app's (`3.5.43`), Svelte's
  `window.__svelte.v` (`5`), and none when the page does not say.
- 15.2: The adapters' bundle starts with `"use strict"`, which rules out
  a rest parameter in the function it is wrapped in; it reads
  `arguments`. React 19's debug stack is sent whole (12 frames at most)
  and the engine takes the first frame that is not a JSX factory
  (`jsxDEV`, `jsx`, `jsxs`, `createElement`, `cloneElement`) nor a
  library's once source-mapped.
- 15.2: Framework places are file names, not URLs, so they are matched
  to the disk on any page (15.1 matches URLs only where source maps are
  resolved).
- 15.2: With a framework's production build, a node stack answers only
  with a source-mapped frame of the app (in one bundle, nothing else
  tells the app's frames from the framework's); else `NoSource` "not a
  dev build". `NoSource`'s `details.reason` is `not_dev_build`,
  `before_stacks`, `stacks_off` or `library_only`. "No source map" is
  not an error: the place is the served script's, with a warning.
- 15.2: Telling "parsed from the HTML" from "made before recording":
  each page and iframe session records whether stacks have been on
  since its document began to load (`node_stacks_since_load`, cleared
  when `open --node-stacks` turns them on mid-document).
- 15.2: `snapshot --source` resolves each ref'd node in its frame's main
  world, runs the adapters on up to 100 nodes per call, reads the
  stacks of the rest with one `DOM.getDocument` per frame (a second one
  would drop the first's node ids), and maps all stacks in one pass. It
  cannot be used with `--diff`. A node nothing answers for has no
  suffix.
- 15.3: A component's place in `components` is its **own** file, at the
  first element it renders (React: the first fiber below it that it
  owns; Vue: its root element, matched in its template; Svelte: its
  first element's `loc`), not `source`'s usage line: the tree answers
  "which file is this component", and the plan's example (`<CartItem>
  src/CartItem.tsx:12`) shows the component's own file. Owners (with a
  locator) keep `source`'s usage places in JSON.
- 15.3: Tree lines show props one level deep (collections counted:
  `items=[3 items]`, `item={…4 keys}`), functions, `children`, `key` and
  `ref` left out, at most 6. Siblings fold when their name, place,
  these props and their subtrees are the same (runs, in order), which
  is what "no differing props" means here: three items with different
  objects fold, as in the plan's example. A React 19 node sends 4 debug
  stack frames (enough for its first frame of the app) so siblings'
  places compare equal. The page reads at most 5,000 components; past
  `--depth` a node counts what is below it (`… 3 components below`).
- 15.3: The fixture apps' `CartItem` holds no state (its quantity is
  the `item` prop, from `App`'s `useState`), and changing the apps would
  move the lines 15.2's tests pin. So the test checks the item's `item`
  prop and `App`'s state (React's `#0` hook, Vue's `items`) before and
  after "Add" instead of a `qty` state in `CartItem`.
- 15.3: With a locator, the component is the element's owner (the one
  whose code made it, as `source` says), else the nearest component
  around it; React's owner fiber is swapped for its on-screen copy
  (`alternate`) when that is the current one, so state after a click is
  fresh. Vue's state is `setupState` without functions, components and
  the `props` alias `<script setup>` exposes, then `data`. Svelte 5
  prints `state: not available (Svelte 5 has no public API …)`; JSON has
  no `props`/`state` and an `unavailable` reason. `--depth` bounds the
  owners there (default 8, as `source`).
- 15.3: Only the tab's main frame is read for the tree; with a locator,
  the element's frame. `NoSource` gains the reasons `no_framework` (no
  React, Vue or Svelte on the page) and `no_component` (no component of
  a dev build rendered the element). The 200-item list is the React 19
  app with `cart.json` routed; the deep tree a page that fakes 41 nested
  Svelte components (`testdata/site/source/deep-svelte.html`).
- 15.4: Breakpoints are set by the script's URL
  (`Debugger.setBreakpointByUrl`), not by `scriptId`: V8 sets them again
  in the next document's script before it runs, so a reload needs no
  pause. V8 names such a breakpoint by its URL, line and column (not its
  condition), so the logpoints at one place share one breakpoint whose
  condition runs each, set again when one comes or goes.
- 15.4: Only scripts with a source map pause before they run
  (`beforeScriptWithSourceMapExecution`, as planned); a script without
  one is bound when `Debugger.scriptParsed` reports it, after its top
  level ran (its functions log from then on). An inline script is its
  document's URL, so `debug/page.html:10` works on a page with no build.
- 15.4: The line used is the first mapped column of the line asked (or
  of the next line with mappings, at most 100 lines on), then where V8
  puts the breakpoint (the next place it can stop), mapped back: a
  comment above a function binds at its first statement. `debug log`
  reports the line it used when it is another.
- 15.4: A place matches a source when one's path segments end with all
  of the other's, or the source is on disk as the file the place names
  (`places::same_file`); the place may be a file name alone (`cart.ts`),
  a source must have two segments to be the end of a longer place.
  Sources compare without their query (`?t=123`). Ambiguity is decided
  at `debug log`, over the sources of the tab's scripts then (more than
  one distinct source: `Ambiguous`, with candidates); a pending
  logpoint takes the first source that matches in a script loaded
  later.
- 15.4: Each expression is compiled (not run) in the tab's main frame
  first (`Runtime.compileScript` of `(() => (EXPR\n))`): a typo fails
  with `BadArgs` instead of a logpoint that never logs. No expression is
  allowed too (`debug log src/App.tsx:30`: was the line reached?).
- 15.4: The condition holds `logpoint.js`, a third bundle built from
  `injected/src/logpoint.ts`, which reuses `components`' value preview.
  Hits are console entries of source `logpoint` with a structured
  `ConsoleEntry.logpoint` (additive), and the report's `logpoint` event
  carries the same. Hits past `--limit` are dropped while the
  breakpoint is taken out; the stop is one console entry.
- 15.4: Logpoints belong to a tab (`--tab`) and go with it when it
  closes. Debugger is on in the page, iframe and worker sessions of the
  tabs with a logpoint that has not stopped (shared and service workers
  of those tabs too), and off elsewhere; `debug clear` turns it off where
  nothing needs it (15.5's locals will be a second reason to keep it).
- 15.4: A pause chrome-cdp did not ask for is resumed, then noted as a
  `warn` console entry of source `debugger`: `paused at a debugger
  statement (src/x.ts:3); resumed` (`a breakpoint` when one a raw `cdp`
  call set was hit). `CallFrame.url` is deprecated and empty in Chrome
  154, so the place comes from the script's `scriptParsed`.
- 15.4: The plain app gained `src/later.ts`, imported when the page
  gets a `later` event (appended to `main.ts`, so the lines 15.2's tests
  pin do not move), for the pending logpoint; `testdata/site/debug/
  debugger.html` has the `debugger;` statement. The ambiguous place is
  the React 19 app's `index.js` (`react/index.js`, `react-dom/index.js`).
  "`debug clear --all` disables Debugger" is checked by the `debugger;`
  statement no longer pausing (no second note).
- 15.5: Locals capture follows each target's own URL (`localhost` and
  loopback documents and workers by default, like source maps), switched
  at `Page.frameStartedNavigating` before the new document runs, as node
  stacks are. Debugger is therefore on in every local page, whether or
  not it has a logpoint: a session's `Mode` says why (logpoints, locals),
  and only logpoints set the instrumentation breakpoint and the binding,
  so a dev server's many modules do not each pause. `debug clear` leaves
  Debugger on where locals need it and says so. `debug locals` takes
  `localhost` too (back to the default) and, with no argument, shows the
  setting.
- 15.5: At a pause the names come from `Runtime.getProperties` of the
  `local`, `closure` and `block` scopes (an inner name hides an outer
  one, 10 a frame), and the values from one
  `Debugger.evaluateOnCallFrame` a frame that runs `logpoint.js`'s new
  `locals` over a closure per name: the same preview as logpoint values,
  and a `let` read before its line shows its error. Pauses in scripts
  without a URL (`eval`, chrome-cdp's own calls) resume at once. At most
  10 reads a second a session; the errors past that resume at once and
  say `too many errors`.
- 15.5: Matching rule: the read waits in its session (16 at most, 10
  seconds) for the `Runtime.exceptionThrown` of the same thrown value (its
  `description`, which holds an error's stack, else its value) whose top
  frame has the same script and line; else the same value and script.
  Frames get locals by index when the function names agree. An entry
  without them has `ConsoleEntry.no_locals` (additive) with the reason,
  which the text prints once per reason. Neither `locals` nor `no_locals`
  is in the output without `--locals`, so `console --json` did not grow.
- 15.5: `setPauseOnExceptions("uncaught")`, as planned, only stops where
  no JavaScript catches the error. In the fixture apps that is the plain
  app and React 18 (its dev build rethrows in a fake event). React 19
  catches a handler's error and calls `reportError`, so the debugger
  never stops; Vue (dev) and Svelte 5 catch it and throw it again from
  their handlers, so it stops in the framework. Those errors say why and
  print `chrome-cdp debug log <top frame's place> <expr>…` instead.
  Pausing on caught exceptions too would cover them, but pauses at every
  caught throw of a page (a parser's `try`/`catch` in a loop): left as a
  follow-up.
- 15.5: Chrome gives initiator stacks async parents only with
  `Runtime.setAsyncCallStackDepth`, which costs the page time per async
  task: it is set (8) where Debugger is on, so on localhost pages and in
  tabs with a logpoint. The "Add" request's stack is synchronous in every
  app (`fetchCart` calls `fetch` before its first `await`), so the async
  boundary is checked on `testdata/site/debug/initiator.html` (a timer,
  then a promise). `StackFrame` gains `async_boundary` on the first frame
  of each async parent; frames without a URL are left out, so the
  innermost frame (`Initiator.url`, `line`, `function`) is now the
  innermost one with a URL. The `network` list maps only first frames,
  and shows `from src/api.ts:13` when a source map gives an original
  place.
- 15.5: With pause-on-exceptions set, V8 also pauses at a failed
  `console.assert` (reason `assert`): it resumes at once, unnoted (the
  console has the assertion). `network` and `network show` gained
  `--sourcemaps`, as `console` and `errors` have, for initiator stacks
  off localhost. No golden shows `network show`'s initiator stack (its
  timing line is not stable); the engine tests check its text.
- 15.6: Message shapes were checked against Vite 8.3.1 (the fixture apps'
  pin): the `vite-hmr` subprotocol; `update` with `updates[].path` and
  `acceptedPath`; the client logs `[vite] hot updated: <acceptedPath>`
  (`… via <path>` for a boundary) with `console.debug`, and `[vite] Failed
  to reload <path>. …` (Vite before 6: `[hmr] …`) with `console.error`,
  but then logs `hot updated` for the same module too, so a failure wins
  its round; with the overlay on (the default) a compile error is only the
  `error` frame. Vite 8 (oxc) sends no `err.loc`: the place is the
  diagnostic header in the message (`╭─[ src/x.tsx?t=1:23:8 ]`), the file
  is `err.id` without its query, and the first line is the one after
  `Transform failed with 1 error:`. `full-reload` has `path: "*"` and the
  edited file in `triggeredBy`; one with `ifFallback`, or for another
  page's HTML, does not reload this page and is ignored.
- 15.6: webpack-dev-server 5 and Next.js are not installed here: their
  shapes come from their clients' sources as documented, not from a run.
  webpack: a socket is its client when one of its first 4 messages is
  `hot`, `liveReload` or `hash` (SockJS `a[…]` batches too); `invalid`
  starts a round, `errors[0]` fails it (`message`, `moduleName`, `loc`
  `12:5-9`), `static-changed` and `[HMR] … Need to do a full reload`
  reload, and `[HMR] Updated modules:` / `[HMR]  - <module>` / `[HMR] App is
  up to date.` apply. Next.js: the `/_next/webpack-hmr` socket; `building`,
  `built`/`sync` with `errors`, `reloadPage`, `serverComponentChanges`, and
  `[Fast Refresh] rebuilding|done|performing full reload`. The fake
  webpack client stands for the manifest fetch with a `fake-modules`
  message.
- 15.6: A round starts with Vite's `update`, webpack's `invalid` or Next's
  `building` and ends in one outcome; what else ends it merges into that
  (a Vite update's files, the compile error and the client's "failed to
  reload"). A full reload is done at the main frame's first `load` of
  another document than the one it came in; reloads merge until then.
  `building` is a state, not an outcome: a timeout names it. A tab keeps
  32 outcomes.
- 15.6: `wait --hmr` answers with the **latest** outcome since the previous
  command began, not the first (after two saves, the page's state is the
  second's), once updates stayed quiet for 200 ms (one save can make two
  rounds; a Vite update's pending files get 500 ms), and never with one an
  earlier `wait --hmr` returned: after a failure, the next wait is for the
  fix. A full reload answers once loaded; the deadline passing during the
  quiet time still answers.
- 15.6: A failure is a new error kind, `HotUpdateFailed` (exit 1, additive),
  with the compiler's line, the place (on disk under the project root), and
  `details.hot_update`. No client is `NotFound` with `details.reason`
  `no_client`: the page's client is known by its socket (or by a `[vite]`,
  `[HMR]`, `[webpack-dev-server]` or `[Fast Refresh]` line of the current
  document).
- 15.6: "Any command warns once": every page command's `Engine::page`
  notes the tab's outcomes after the last one a command reported
  (`hmr::note_since_previous`), and the daemon runs each call in
  `engine::noting` and adds them to its output as warnings and
  `hot_update` events (to an error as `meanwhile:` notes). An action's
  report lists what came while it ran as `! …` lines and marks them
  reported. Outside `noting` (engine tests, the runner's internal calls)
  nothing is reported. File names print as project paths when the file
  is on disk, on any page (a dev server names its own files).
- 15.6: The fake servers are `/hmr/vite?channel=…` and
  `/hmr/webpack?channel=…` (one channel per test, since tests share one
  server) and `POST /hmr/send?channel=…`; `testdata/site/hmr/vite.html`
  and `webpack.html` log what the real clients log, and set
  `hmrConnected` once greeted (a test that sends before Chrome reported the
  socket would find no client). The gated test copies `react19` and
  `shared/` to a temporary directory, links the app's `node_modules`, and
  runs `node node_modules/vite/bin/vite.js`; it passes here. There is no
  CI workflow any more (removed in `c5659a3`), so nothing sets
  `CHROME_CDP_NODE_TESTS=1` yet.
