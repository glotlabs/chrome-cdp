# Plan 06 — Observability

Status: IN PROGRESS (6.1-6.3 done)
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

- `screenshot [loc] [--full] [--format png|jpeg|webp] [--quality N] [-o path]
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
- 6.2: Requests of dedicated, shared and service workers are not recorded yet (workers get no `Network.enable`), and there is no `network --follow`; both are follow-ups.
- 6.3: One `Page.captureScreenshot` per image, with a `clip` in page coordinates whose `scale` sets the size: `--scale css` and the `--max-size` downscale are rendered by Chrome at that size, so no image crate is needed. The device pixel ratio is read from the page (`devicePixelRatio`); `Page.getLayoutMetrics`' device-pixel fields ignore device emulation.
- 6.3: A viewport screenshot is the layout viewport, which leaves out classic scrollbars: every image pixel (at `--scale css`) is a CSS pixel of the page. `--full` is `cssContentSize` with `captureBeyondViewport`; a 20,000px page is one capture. `--max-size` (1568 by default) applies to `--full` too, and a capture shrunk below half size gets a warning naming `--max-size 0` or a locator.
- 6.3: An element screenshot waits (within `--timeout`) for the locator to match one rendered element, like an action waits for its element, then scrolls it into view with `DOM.scrollIntoViewIfNeeded`, and each `<iframe>` it is in to show its box (an out-of-process frame's scroll reaches its parent only later, as actions found), and clips to its `getBoundingClientRect` plus the offsets of the iframes it is in (the same as `box`, whose `frame_offset` moved to `Engine`). `captureBeyondViewport` is set only when the box goes past the viewport.
- 6.3: `--mask` and `--annotate` draw into the page just for the capture: a zero-size `<chrome-cdp-overlay>` with a closed shadow root, appended to each frame's document, with boxes positioned against the host's own containing block; it is removed after the capture, also when it fails. Masks paint `#ff00ff` over every match (as Playwright does). Labels mark the refs `snapshot -i` would show (reading them counts as the tab's last snapshot for `--diff`), drawn in each element's own frame; the output lists the refs whose boxes are in the image (the text form the first 30).
- 6.3: Files without `-o` go to `<session dir>/shots/<n>.<ext>` and `<session dir>/pdfs/<n>.pdf` (the daemon's `Engine::set_output_dir`), numbered on from the highest file there and created with `create_new`, so a daemon restart or two tabs never overwrite a capture. `-o` is made absolute by the CLI, its directories are created, and its extension picks the format unless `--format` is given (a mismatch is a warning). `-o -` saves to the session directory, and the CLI copies the file to stdout and deletes it; with `--json` it is a `BadArgs`.
- 6.3: `screenshot` and `pdf` have no `--max-chars`: they print a path and a line or two (they are in the checklist's fixed-size list).
- 6.3: `pdf --format` takes Letter (Chrome's default), Legal, Tabloid, A3, A4 and A5 in any case; `--margin` takes one to four CSS-ordered lengths in `in`, `cm`, `mm` or `px`. A launched headed browser is refused before the call; a connected one when Chrome answers that `printToPDF` is not implemented. The page count comes from the file's `/Type /Page` objects.
- 6.3: The `--annotate` image is compared with a golden PNG (`crates/engine/tests/golden/annotate.png`, `UPDATE_GOLDEN=1` rewrites it): at most 1% of the pixels may differ by more than 64 in a channel, which leaves room for font rendering. Tests decode PNGs with the `png` crate (a dev-dependency).
- 6.3: The criterion "screenshot of a 20k-px-tall page works on both OSes" stays unticked: it passes on Linux (`viewport_element_and_full_page` checks the last band's pixels at full size), but macOS has not been run.
