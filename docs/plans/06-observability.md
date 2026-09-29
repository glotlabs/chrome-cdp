# Plan 06 — Observability

Status: DONE (6.1-6.6; the 1-hour RSS soak is built, `crates/chrome-cdp/tests/soak.rs`, and runs with the benchmarks after all plans; the 20k-px screenshot waits for a macOS run, and `inspect`'s success path for a machine with a display)
Depends on: 03 (can proceed in parallel with 04/05)

## Goal

Let the agent see everything a developer sees in DevTools, cheaply and
without asking in advance. From the moment a tab exists, the daemon records
console output, errors and network activity into bounded buffers, so
`chrome-cdp errors` works retroactively.

## Milestone 6.1 — Console & errors

- Always on: `Runtime.consoleAPICalled`, `Runtime.exceptionThrown`,
  `Log.entryAdded` (browser-side messages such as CSP violations,
  intervention warnings and failed resource loads).
- A ring buffer per tab (default 1,000 entries, configurable). Each entry has a
  monotonic sequence number, timestamp, level, text (args formatted like
  DevTools; objects previewed via `RemoteObject.preview` and never fully
  fetched), source location, and stack (top 5 frames).
- `console [--level error|warn|info|debug] [--since last|<seq>|<duration>]
  [--grep re] [--limit 50] [--follow]`.
  - `--since last` (the default) shows only entries the agent has not seen
    yet. The daemon keeps a read cursor per tab per session, so repeated calls
    don't repeat output.
  - `--follow` streams until Ctrl-C or `--timeout`.
- `errors` is sugar for uncaught exceptions + `console.error` + failed network
  requests on the page origin.
- Source maps: if `--sourcemaps` is set (or it is a `localhost` page), fetch
  the `.map` files and resolve stack frames to the original source. This is a
  big win for agents fixing their own dev server's code. Use the
  `sourcemap` crate. Cache per URL.

## Milestone 6.2 — Network log

- Always on: `Network.enable` with buffer limits. Record the request
  (method, url, type, initiator summary, headers on demand), the response
  (status, mime, size, timing, from-cache, protocol), failures (`errorText`,
  `blockedReason`, CORS error details) and WebSockets (frames summarised). Use a
  ring buffer per tab (default 2,000).
- `network [--failed] [--type xhr,fetch,document,…] [--url glob] [--method]
  [--status 4xx] [--since last] [--limit 50]`. One line each:
  `#42 POST 500 /api/login xhr 12KB 340ms`.
- `network show <#id> [--headers] [--body] [--request-body]`: fetch the body
  lazily via `Network.getResponseBody`. Pretty-print JSON and truncate with
  `--max-chars`. Bodies are available only while Chrome retains them; if one
  was evicted, say so.
- `network clear`.

## Milestone 6.3 — Screenshots & PDF

- `screenshot [loc] [--full [loc]] [--format png|jpeg|webp] [--quality N] [-o path]
  [--scale css|device] [--omit-background] [--mask <loc>...]`.
  - The default output path is `<session dir>/shots/<n>.png`, and the command
    prints the path. It never writes binary to stdout unless `-o -`.
  - Agents with vision read the file. Default to CSS-pixel scale and a max
    dimension (`--max-size 1568` by default, a size that suits vision models;
    downscale it) so images don't waste tokens.
  - Element screenshots clip to the element box after scroll-into-view.
    `--full` uses `captureBeyondViewport` with the content size.
  - `--annotate` overlays ref labels (`e12`) on the interactive elements in
    the image, like Set-of-Marks. This helps vision agents map pixels to
    refs.
- `pdf [-o path] [--format A4|Letter] [--landscape] [--background] [--margin]`
  via `Page.printToPDF` (headless only; in headed mode, give a clear error).

## Milestone 6.4 — Evaluate & raw access

- `eval <js> [--in <loc>] [--frame f] [--await] [--world main|isolated]`.
  - The expression is evaluated with `returnByValue`, `awaitPromise` and
    `userGesture: true`.
  - With `--in e12`, the element is bound as `el` (via `callFunctionOn`).
  - Output: JSON-ish, truncated. Exceptions show the message and a mapped
    stack. `eval -` reads the script from stdin (easier quoting for agents).
- `cdp <Domain.method> [json]` (from plan 02) gains `--tab` and
  `--listen <Event> [--count N]` to capture raw events.

## Milestone 6.5 — Tracing, HAR, performance

- `har start [--content embed|omit]` / `har stop -o file.har`: build the HAR
  1.2 file from the network log.
- `trace start [--screenshots] [--categories …]` / `trace stop -o trace.json`:
  a Chrome performance trace (`Tracing.start` with `ReturnAsStream`, then read
  it via `IO.read`). The file opens in the DevTools Performance panel or
  Perfetto.
- `perf`: key metrics for the current page (`Performance.getMetrics`, plus
  navigation timing, LCP, CLS and INP from a small injected
  PerformanceObserver) as one compact block.
- `coverage start|stop` (JS/CSS, `Profiler` + `CSS.startRuleUsageTracking`):
  the output is a summary of unused bytes per file. Stretch goal for v1.

## Milestone 6.6 — Headed-mode helpers

- `highlight <loc>` (`Overlay.highlightNode`, for 2 s).
- `inspect`: open DevTools for the tab (headed only). These are for humans
  pairing with an agent.

## Tests

- Fixture pages that log at every level, throw sync/async/promise-rejection
  errors, produce CSP violations, have failing fetches (404, CORS, DNS,
  aborted), large JSON responses, and WebSocket echo.
- Cursor semantics: `console` twice shows the new entries only; `--since 0`
  shows all.
- Screenshot sizes: viewport, element, full page, and max-size downscale.
  `--annotate` output is golden-image compared with a tolerance.
- HAR validates against the HAR 1.2 schema.
- The source map fixture: an esbuild bundle with a map. The error stack shows
  the original `.ts` file and line.

## Acceptance criteria

- [ ] Buffers are bounded. The daemon's RSS stays under 150 MB after a 1-hour
      soak test on a page that logs 100 msgs/s.
- [x] `errors` after a failed action always surfaces the root-cause exception
      on the fixtures.
- [ ] Screenshot of a 20k-px-tall page works on both OSes.

## Decisions log

_(append here)_
- 6.1: The console buffer grew from milestone 5.5's error log (`engine::console`, now `engine::observe::console`), fed by the same `watch_host` events; the post-action report reads its errors from it. `Log.enable` joined `Runtime.enable` in every page and iframe session's setup, before the target resumes.
- 6.1: Dedicated workers get `Runtime.enable` and `Log.enable` too, and log into their tab's console (`worker` names the script). A worker's `Log` repeats its console calls with `source: "worker"`; those are skipped. Shared and service workers have no tab and are not recorded yet.
- 6.1: Entries hold only text: arguments are formatted on arrival from `RemoteObject.preview` (`%s`/`%d`/`%o`/`%c`, `{a: 1}`, `[1, 2]`, `Map(1) {'k' => 1}`), never fetched. Text is kept up to 4,000 chars and 5 stack frames. For `console.error(err)` the stack is the error's own (parsed from its `stack` text), not the call's: where it was thrown is the root cause. `Runtime.exceptionRevoked` (a rejection handled late) removes the entry.
- 6.1: `time_ms` is when the daemon received the entry (not CDP's timestamp, whose clock differs between `Runtime` and `Log`), so `--since 30s` compares one clock.
- 6.1: Read cursors are per tab and per command: `console` and `errors` each show what they have not shown, so reading one does not hide entries from the other. A call moves its cursor to the newest entry it showed.
- 6.1: `--limit` (default 50) keeps the newest entries; a line says how many earlier ones were left out and the `--since <seq> --limit <n>` that shows them. Entries the ring buffer dropped before they were read are counted in `dropped` and a warning naming `open --console-buffer`.
- 6.1: `--level` defaults to `info`: `console.debug` and Chrome's verbose messages (violations) are hidden, as in DevTools' default levels. A level shows itself and everything more severe.
- 6.1: `errors` is every error-level entry, except failed loads (Chrome's `Log` `network` messages, which already name the URL and status) from other origins and `/favicon.ico`. Taking failed loads from the console rather than merging in the network tracker keeps one sequence, one cursor and one `--since` for `errors`; the CORS explanation Chrome logs is an error of the page's origin and stays.
- 6.1: `--follow` is a long poll, not a streaming RPC: the CLI asks with `wait_ms` (at most 25 s), the daemon answers when an entry arrives, and the CLI prints each non-empty batch (in `--json`, one envelope per batch) until Ctrl-C, or `--timeout` if given.
- 6.1: `console` and `errors` take no tab lock and do not count as a page command (they would move `wait --request`'s "previous command" mark), so they work while an action runs or a dialog is open.
- 6.1: Source maps are loaded through Chrome with `Network.loadNetworkResource` on the tab's main frame (as DevTools does: the page's cookies apply, no CORS), not by the daemon. The map comes from the `SourceMap` header or the script's last `sourceMappingURL` comment (inline `data:` maps too); `Debugger.scriptParsed` would give it directly but needs the `Debugger` domain on in every page. Outcomes, misses too, are cached per script URL for the browser's life; URLs of the tab's documents are never loaded (inline scripts). A mapping on an earlier generated line is not used. Frames keep their generated function name: a map's names belong to identifiers, not to the enclosing function.
- 6.1: The acceptance criterion on RSS after a 1-hour soak is left unticked: deferred, benchmarks run after all plans. Buffers are bounded by construction (entries, text length and stack depth).
- Close-out: the soak test is `crates/chrome-cdp/tests/soak.rs` (`#[ignore]`, page `console/soak.html`: 100 logs, an error and a request a second; `CHROME_CDP_SOAK_SECS` shortens the hour). It samples the daemon's RSS with `ps` and checks the console buffer's size at the end; only its wiring was run (with 0 seconds). Deferred: benchmarks run after all plans.
- 6.1: Following up on plan 03 and 05 notes: a wait or navigation timeout adds `console errors: N unread; chrome-cdp errors shows them` (and `details.console_errors`), and the report's `… and N more console errors` line names `errors`.
- 6.2: The network log (`engine::observe::netlog`) sits beside plan 03's request tracker (`engine::network`, for idle, `wait --request` and the report), fed the same `Network` events in `tabs::watch_host`; they stay separate because the tracker's in-flight and quiet-time bookkeeping serves waits, not history. Records are keyed by session and request id while live; ids (`#42`) count per tab and survive `network clear`.
- 6.2: Each redirect hop is its own record (the 3xx with `redirect` naming the next URL), as in DevTools. `time_ms` is when the daemon saw the request start (one clock with `console`); durations use Chrome's monotonic timestamps.
- 6.2: Headers are those of `requestWillBeSent` and `responseReceived`; the `…ExtraInfo` events (raw headers with cookies) are not merged yet, since they may arrive before their request and are ambiguous across redirect hops. Records keep up to 100 headers a side, 4,000 URL chars, and request bodies up to 8KB; longer bodies come from `Network.getRequestPostData` when asked for.
- 6.2: `Network.enable` bounds what Chrome keeps (64MB of bodies a session, 10MB each, request bodies up to 64KB in events), and `Network.configureDurableMessages` (not `enable`'s deprecated `enableDurableMessages`) keeps response bodies in the browser process: without it Chrome dropped them on any navigation, even same-origin, so the response to a submit was gone by the time the agent asked. It is sent before `Network.enable` but its answer is not awaited with the setup steps: in a new popup Chrome answers it much later, which held up the tab's readiness.
- 6.2: Chrome sends `loadingFinished` for a `fetch` only once the page reads the body, so a response whose body is never read stays `pending` with its status known. Lines show the status once headers are in (`#6 GET 404 /missing.json fetch`), and `network show` says "body still loading".
- 6.2: `--failed` means no response (`state: failed`) or a status of 400 or more, pending body or not; requests canceled before a response (aborted fetches, navigations) are not failures but are listed as `canceled`.
- 6.2: `--since last` works like `console`'s cursor, plus: a request shown while pending is shown once more when it ends (no response to a slow request goes unseen). `--since <id>` and `--since 30s` work as in `console`.
- 6.2: `--type` takes Chrome's resource types in lower case (`xhr`, `fetch`, `document`, …, `websocket`); a custom parser names them in its error, since the CLI's error rendering drops clap's possible-values list. `--status` takes `404`, `4xx` or `500-599`, comma-separated, as does `--type`.
- 6.2: A WebSocket is one record: status 101 while open, frames counted with their bytes, and the last 20 frames (200 chars of text each) for `network show`; `size` is the payload bytes.
- 6.2: `network -q` prints the ids; `network show --body -q` prints just the body (for `> file`). JSON bodies are re-indented as they came (parsing into a `Value` would sort their keys); binary bodies are not shown.
- Close-out: `network show -o <path>` saves the response body, decoded (an image as its bytes), and prints the path (`-q`: only the path); the text form says `response body: saved to <path> (12KB)` and `--json` has `response_body.path`. No body to save (pending, failed, evicted) is a `NotFound` and no file. `-o -` is refused: `--body -q` already prints a text body, and binary on a terminal helps nobody.
- 6.2: Requests of dedicated, shared and service workers are not recorded yet (workers get no `Network.enable`); a follow-up.
- Close-out: `network --follow` is `console --follow`'s long poll (the CLI loop is now one `follow` helper for both): `wait_ms` waits on a change count the tab's `NetLog` keeps, so a pending request that ends is new, as for `--since last`, and a pending one is printed twice (pending, then ended). The `network show` hint comes with the first batch only.
- Close-out: the report's failed-request lines name the request's network log id (`! request failed: #42 POST /api/save (500)`, `id` in the `request_failed` event), so `network show 42 --body` follows at once. The failure comes from plan 03's tracker, which now keeps the request's session and CDP id; the log's newest record with both (a redirect's last hop) gives the id. The tracker stays: it serves waits, not history.
- 6.3: One `Page.captureScreenshot` per image, with a `clip` in page coordinates whose `scale` sets the size: `--scale css` and the `--max-size` downscale are rendered by Chrome at that size, so no image crate is needed. The device pixel ratio is read from the page (`devicePixelRatio`); `Page.getLayoutMetrics`' device-pixel fields ignore device emulation.
- 6.3: A viewport screenshot is the layout viewport, which leaves out classic scrollbars: every image pixel (at `--scale css`) is a CSS pixel of the page. `--full` is `cssContentSize` with `captureBeyondViewport`; a 20,000px page is one capture. `--max-size` (1568 by default) applies to `--full` too, and a capture shrunk below half size gets a warning naming `--max-size 0` or a locator.
- 6.3: An element screenshot waits (within `--timeout`) for the locator to match one rendered element, like an action waits for its element, then scrolls it into view with `DOM.scrollIntoViewIfNeeded`, and each `<iframe>` it is in to show its box (an out-of-process frame's scroll reaches its parent only later, as actions found), and clips to its `getBoundingClientRect` plus the offsets of the iframes it is in (the same as `box`, whose `frame_offset` moved to `Engine`). `captureBeyondViewport` is set only when the box goes past the viewport.
- 6.3: `--mask` and `--annotate` draw into the page just for the capture: a zero-size `<chrome-cdp-overlay>` with a closed shadow root, appended to each frame's document, with boxes positioned against the host's own containing block; it is removed after the capture, also when it fails. Masks paint `#ff00ff` over every match (as Playwright does). Labels mark the refs `snapshot -i` would show (reading them counts as the tab's last snapshot for `--diff`), drawn in each element's own frame; the output lists the refs whose boxes are in the image (the text form the first 30).
- 6.3: Files without `-o` go to `<session dir>/shots/<n>.<ext>` and `<session dir>/pdfs/<n>.pdf` (the daemon's `Engine::set_output_dir`), numbered on from the highest file there and created with `create_new`, so a daemon restart or two tabs never overwrite a capture. `-o` is made absolute by the CLI, its directories are created, and its extension picks the format unless `--format` is given (a mismatch is a warning). `-o -` saves to the session directory, and the CLI copies the file to stdout and deletes it; with `--json` it is a `BadArgs`.
- Close-out: `pdf` gained `--pages` (Chrome's `pageRanges`: `1-3,5`, `3-`) and `--css-page-size` (`preferCSSPageSize`; without it Chrome scales a page's `@page` size onto `--format`'s paper). A range Chrome refuses is a `BadArgs` naming `--pages`. Streaming large PDFs (`transferMode: ReturnAsStream`) is left for later.
- 6.3: `screenshot` and `pdf` have no `--max-chars`: they print a path and a line or two (they are in the checklist's fixed-size list).
- 6.3: `pdf --format` takes Letter (Chrome's default), Legal, Tabloid, A3, A4 and A5 in any case; `--margin` takes one to four CSS-ordered lengths in `in`, `cm`, `mm` or `px`. A launched headed browser is refused before the call; a connected one when Chrome answers that `printToPDF` is not implemented. The page count comes from the file's `/Type /Page` objects.
- 6.3: The `--annotate` image is compared with a golden PNG (`crates/engine/tests/golden/annotate.png`, `UPDATE_GOLDEN=1` rewrites it): at most 1% of the pixels may differ by more than 64 in a channel, which leaves room for font rendering. Tests decode PNGs with the `png` crate (a dev-dependency).
- 6.3: The criterion "screenshot of a 20k-px-tall page works on both OSes" stays unticked: it passes on Linux (`viewport_element_and_full_page` checks the last band's pixels at full size), but macOS has not been run.
- 6.4: `eval` is one `Runtime.evaluate` in REPL mode (`replMode`), as DevTools' console runs what is typed: statements are fine and the last one's value is the result, top-level `await` works, `userGesture` is set. There is no `--await` flag: a promise is always awaited (REPL mode does not await a promise the script ends with, so the finishing call does), and there is nothing for the flag to switch.
- 6.4: The script runs inside a block (`{\n…\n}`) so its `let`s and `const`s do not stay in the page's global scope, where a later page script declaring the same name would fail. A script that looks like an object literal (`{a: 1}`) is first tried in parentheses, as DevTools does; a syntax error (an exception with no stack: nothing ran) falls back to the block. A `//# sourceURL=eval` names the script's stack frames, their lines shifted back to the script's own.
- 6.4: A value that is a function is called (with `el`), Playwright style, so `el => el.value` and `() => …` work. An object value is turned into JSON text in the page by one `callFunctionOn` (`observe/eval.js`): `returnByValue` alone gives `{}` for DOM nodes and fails on cycles, and parsing into a `Value` would sort the keys, so the text form prints the page's text (keys in order) and `--json` has the parsed value. DOM nodes become `button#save.primary`, cycles `"[Circular]"`, `NaN`/bigints/symbols strings; a top-level string prints as it is. Refs for returned nodes are a follow-up (done: see the entry on eval refs below).
- 6.4: `--in` resolves its locator with the locator engine (no auto-wait, like the read commands), moves the element into the chosen world with `DOM.resolveNode`, and hands it to the script through a `Symbol.for` slot on `globalThis` that the wrapper takes and deletes first thing: `Runtime.evaluate` cannot bind arguments, and `callFunctionOn` cannot take statements or top-level `await` without an `eval()` that a page's CSP may forbid. `--in` and `--frame` conflict: the element's frame is where the script runs.
- 6.4: An exception is an `EvalError` (exit 1): "the script threw …" or "the script does not parse: …", up to 5 `at:` notes, and the stack in `details`; frames in page scripts are resolved with source maps as `errors` resolves them (`--sourcemaps`, on for localhost; `Engine::map_stack` is shared). Hints teach the common mistakes: `return` at the top level, `el` without `--in`, and shell quoting (`eval -`) for syntax errors. A promise that does not settle within `--timeout` is a `Timeout` (exit 3); a navigation that destroys the context mid-script says so.
- 6.4: Each `eval` gets its own object group, released after it answers: a shared group released in the background freed the next call's objects.
- 6.4: `cdp --tab` (and `--frame`) now send the call to a page unless `--target` is given; before, `--tab` without `--target page` was ignored. The method is optional with `--listen`, which only listens then.
- 6.4: `--listen` subscribes to the call's session before the call, so no event the call causes is missed, and listens after it without the tab's lock (other commands on the tab run meanwhile and may be what fires them) and outside the dialog interrupt (a dialog may be the event). Only that session's events are seen: a cross-site iframe has its own (`--frame` picks it). `--count` defaults to 1; `--count 0` keeps events until `--timeout` (10,000 at most). With a count and no event in time it is a `Timeout` whose details keep the call's result; fewer than asked for is a warning. `--json` `data` is then `{result, events: [{method, params}]}` (the schema is an untagged `CdpData`), the text one `Domain.event {params}` line per event.
- 6.5: `har` needs no recording mode: every tab's network log records anyway, so `har start` notes each tab's newest request id and `har stop` writes the records after it, of every tab (tabs opened in between from their first request) or of the one `--tab` named. Each tab is one HAR page (the log has no navigation boundaries). A recording longer than the ring buffer loses its oldest requests, with a warning naming `open --network-buffer`. Starting again while recording keeps the marks and takes the new `--content`; stopping without a start is a `BadArgs` naming `har start`; a failed save leaves the recording running.
- 6.5: HAR bodies are asked of Chrome at `har stop` (`Network.getResponseBody`, 8 at a time): text as it is, binary in base64 with `encoding`; request bodies the log did not keep (over 8KB) come from `Network.getRequestPostData`. Bodies Chrome no longer has are left out and counted in a warning, as are requests still loading. Fields the log lacks are HAR's "unknown" (`headersSize`/`bodySize` -1, timings -1 without timing); DevTools' `_resourceType`, `_transferSize`, `_error` and `_webSocketMessages` (the log's last 20 frames) are added. The test checks the har-schema's required fields and types by hand instead of adding a JSON Schema validator as a dependency.
- 6.5: (the 6.2 follow-up, for HAR's cookies) `Network.requestWillBeSentExtraInfo` and `responseReceivedExtraInfo` now replace a record's provisional headers with the raw ones (`Cookie`, `Set-Cookie`, …). One that finds no record wanting it waits per request id (a response's also with its status, which keeps redirect hops apart; 256 requests at most) for the next record or response; one that comes late looks back up to 200 records.
- 6.5: `trace` runs on the tab's session, as DevTools and Puppeteer do (the trace opens on that page), with Puppeteer's categories plus `loading`; `--categories` replaces them (`-name` excludes) and `--screenshots` adds the film strip, which works headless too. `trace stop` copies the `ReturnAsStream` JSON to the file in 1MB `IO.read` chunks, never holding it whole. Chrome can gzip only when told at the start, before the file name is known, so chrome-cdp gzips a `.gz` path itself (`flate2`, already in the tree through `zip`). The recording ends even when saving fails: Chrome's trace is over by then.
- 6.5: `perf` reads a `PerformanceObserver` (`observe/perf.js`) that every top-level document gets in chrome-cdp's isolated world before its own scripts (one more `addScriptToEvaluateOnNewDocument` in each target's setup); isolated worlds see the page's performance timeline. It keeps the latest LCP, CLS as web-vitals computes it (session windows of 1s/5s, shifts after input left out), each interaction's longest event (INP: the 98th percentile of those, as web-vitals picks it), and long tasks (TBT: their time over 50ms after FCP). Chrome gives pointer events no `target`, so an interaction's target (and event name) comes from its click or key event. A page loaded before chrome-cdp attached gets the observer on demand and has what the browser buffered, where Event Timing keeps only interactions of 104ms or more; `from_start` says so, with a warning.
- 6.5: `Performance.getMetrics` durations (script, layout, style) only count while the `Performance` domain is on, which would cost every page, so `perf` turns it on just for the call and uses only its counters (heap, listeners, all of them in `--json`); long tasks stand in for the durations. Right after `Performance.enable`, Chrome at times answers without the V8 heap and listener counts, so it is asked again (up to 5 times, 20ms apart). LCP, CLS and INP are rated by the Web Vitals thresholds (2.5s/4s, 0.1/0.25, 200ms/500ms). `-q` prints the LCP.
- 6.5: `coverage` was a stretch goal and is in: V8's precise block coverage (`Profiler`, `callCount: false`, `detailed: true`; `Profiler` joined `domains.txt`) and CSS rule usage tracking (with `DOM` on, which `CSS` needs; all turned off again at stop), on the tab's own session only. A stylesheet's size comes from `CSS.styleSheetAdded`, collected from before `CSS.enable`. A script offset counts as used when the innermost block range around it ran. Sizes are in characters, as DevTools' Coverage panel counts them. Scripts without an http(s) or file URL (chrome-cdp's own, `eval`) are left out; inline scripts and styles are summed per document. `coverage stop` lists the files with the most unused bytes first (`--limit 20`).
- 6.5: `har stop` and `trace stop` save as `screenshot` and `pdf` do: a numbered file in the session directory (`hars/`, `traces/`) unless `-o` names one; `save::create` is the streaming variant. `har start`, `har stop`, `trace start`, `trace stop`, `perf`, `coverage start`, `highlight` and `inspect` print a line or a fixed block, so they are in the checklist's fixed-size list; `coverage stop` has `--limit` and `--max-chars`.
- 6.6: `highlight` finds its element like the read commands (no auto-wait), scrolls it into view, turns on `DOM` and `Overlay` (which needs `DOM`) and calls `Overlay.highlightNode` with DevTools' colours and tooltip; after `--duration` (2s; 0 keeps it) a timer hides it and turns them off again, unless a later highlight replaced it or `coverage` needs `DOM` on that session. The element gets a ref, as an acted-on element does. It works headless (screenshots show it, which the test checks by pixel), with a warning that only screenshots do.
- 6.6: `inspect` calls `Target.openDevTools` with DevTools' panel ids behind the names users know (`application` → `resources`, `performance` → `timeline`, `memory` → `heap-profiler`, `recorder` → `chrome-recorder`). Chrome opens DevTools headless too, invisibly, so a launched headless browser (or a connected one whose product is `HeadlessChrome`) is refused before the call with a hint to `open --headed --restart`. DevTools' own target does not become a tab (checked by hand). Only the refusal is tested: the dev machine has no display for a headed browser.
- Follow-up: the tab, worker and browser target watchers (which feed the console log, the network log and the in-flight tracker), `cdp --listen` and the navigation watch (`Page.` events only) read lossless streams (`Session::events_lossless("")`) instead of the 1024-event broadcast, so no event is ever skipped. With nothing lost there is no dropped-events count to report; `ConsoleData::dropped` and the network log's `dropped` still count only entries evicted from their ring buffers. The queues are unbounded, which is safe because these watchers never wait on anything but their next event.
- Follow-up (eval refs): elements of the frame's document in an `eval` value get refs from the tab's registry (`button#save.primary [e14]`), so a query leads to an action. `eval.js` returns its answer by reference with the elements in a list, marking each one's place in the JSON text with a per-call token that the engine swaps for the ref. `DOM.describeNode` gives the backend ids, and the bundle's `describe` in the isolated world gives the role and name, so the role+name fallback works as it does for `find` and ambiguity errors (engine B). Detached nodes, nodes of other documents and text nodes get none.
- Follow-up (eval refs): at most 50 elements per call get refs (`MAX_REFS`), a fixed cap with no flag: each costs about three CDP calls, and `.slice(50)` gets the rest. The rest print without one, a warning says how many, and `--json` adds `refs` and `refs_cut`. A failure while giving refs (or a new document meanwhile) drops the refs, not the value.
- Follow-up (`--full` in a container): a page whose content size fits its viewport but that scrolls inside a container gets that container stretched out: the scrolling element (overflow `auto`, `scroll` or `overlay` on an axis with more than 1px to scroll, not the document's scrolling element) that shows the largest area in the viewport, if that is at least a quarter of it, since a sidebar or a code block must not win over the main pane.
- Follow-up (`--full` in a container): the viewport grows (`Emulation.setDeviceMetricsOverride`, keeping the emulated viewport's dpr, mobile flag, screen and orientation) by what the container has to scroll, up to 3 times since a layout may not give it all the new height; content that grows with the viewport gets a warning instead of an endless loop. Growing the viewport rather than restyling the container is what Playwright's docs recommend and leaves the page's CSS alone.
- Follow-up (`--full` in a container): afterwards the viewport the session emulates is set again (or the override cleared), and if the page's `innerWidth`/`innerHeight`/`devicePixelRatio` then differ from before (a raw `cdp Emulation.setDeviceMetricsOverride` set them), that size is set again, since the engine does not know about such overrides.
- Follow-up (`--full` in a container): `--full LOC` takes an optional value (clap `default_missing_value = ""`) rather than a new flag, since `--full` with an element was an error before; a container in an iframe is refused (growing the top viewport does not grow it), and one that does not scroll gives a warning and an ordinary `--full` capture.
