# Plan 06 — Observability

Status: IN PROGRESS (6.1 done)
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
