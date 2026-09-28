# Plan 07 — Network control, emulation & state

Status: IN PROGRESS (7.1 done)
Depends on: 03 (parallel with 04–06)

## Goal

Let agents shape the environment the page runs in: mock APIs, simulate
devices and bad networks, and save and restore logged-in state. All settings
are **session-scoped and sticky**. They apply to existing tabs and are
re-applied to every new tab, popup and OOPIF *before* its scripts run (via
the auto-attach pause from plan 03).

## Milestone 7.1 — Request routing (mocking)

This is the imperative, per-rule routing engine. Plan 11 builds file-based mock
sets, stateful mocks, and request journals on top of it, so design the engine
so that rules can be added in bulk per browser context, and so that it records
every routed request and the rule that handled it.

- Use the `Fetch` domain with request-stage patterns (and response-stage
  patterns when a rule needs the real response).
- `route <url-glob|/regex/> [--method M] [--resource-type T]` with one action:
  - `--status 200 --body '<text>' | --body-file f | --json '<json>'
    [--header K:V]...` (fulfill);
  - `--abort [failed|blockedbyclient|timedout|…]`;
  - `--delay 2000` (then continue);
  - `--set-header K:V`, `--remove-header K`, `--rewrite-url <url>` (continue
    modified);
  - `--patch-json '<jq-like merge>'`: fetch the real response and deep-merge
    the patch. Useful for "what if the API returns an empty list".
  - `--times N`: the rule expires after N matches.
- `routes` lists rules with hit counts. `unroute <id|--all>` removes them.
- `block <glob>...`: sugar for `--abort`, with presets `--block ads`,
  `--block images`, `--block fonts`, `--block media` for speed.
- Rules are evaluated in the daemon. The first matching rule wins, in order of
  addition. Unmatched requests continue immediately; measure the overhead and
  keep it under 1 ms per request. Enable `Fetch` only while at least one rule
  exists, so it has zero cost otherwise.
- Service workers can bypass `Fetch`. Provide `--bypass-service-worker`
  (`Network.setBypassServiceWorker`), and turn it on automatically while
  routes exist. Document this.

## Milestone 7.2 — Emulation

- `emulate <device>`: a built-in device table (about 40 common phones and
  tablets, generated from Playwright's MIT/Apache device descriptors with
  attribution). Each entry sets viewport, DPR, `isMobile`, `hasTouch` and UA.
  `emulate --list`. `emulate reset`.
- `viewport <W>x<H> [--dpr N] [--mobile]`: `Emulation.setDeviceMetricsOverride`.
  In headless mode, the default viewport is **1280×800**, fixed and
  documented.
- `geo <lat,lon> [--accuracy]` (grants the permission automatically), `tz
  <IANA>`, `locale <bcp47>` (`Emulation.setLocaleOverride` + Accept-Language),
  `ua <string>`.
- `media [--color-scheme dark|light] [--reduced-motion reduce] [--type print]
  [--forced-colors active]`.
- `offline on|off`, `throttle <preset|custom>` (`slow-3g`, `fast-3g`, `4g`,
  and `--latency --down --up`), `cpu-throttle <rate>`.
- `permissions grant|deny|reset <name...> [--origin]` (`Browser.setPermission`):
  geolocation, notifications, clipboard-read/write, camera, microphone and
  midi. Fake media streams come from the launch flags
  `--use-fake-device-for-media-stream` and `--use-fake-ui-for-media-stream`,
  on when `--fake-media` is given.
- `time freeze <iso> | time advance <ms> | time reset`
  (`Emulation.setVirtualTimePolicy`, or a `Date`/timer override init script).
  Needed to test time-dependent UIs deterministically. Evaluate which
  approach is robust, and record the result.

## Milestone 7.3 — Init scripts & page-level injection

- `init-script add <file|--code js> [--world main|isolated]` / `list` /
  `remove`: `Page.addScriptToEvaluateOnNewDocument` on all current and future
  targets.
- `expose <name>` (stretch goal): `Runtime.addBinding`, so page JS can call
  back. Calls appear in the `console` log as `binding <name>(args)`.

## Milestone 7.4 — Cookies, storage & auth state

- `cookies [--url u] [--domain d]`, `cookies set <name>=<value> [--domain
  --path --secure --httponly --samesite --expires]`, and `cookies clear`
  (`Storage.getCookies`/`setCookies`, browser-wide).
- `storage local|session [get <key>|set <k> <v>|clear|list] [--origin]` via
  `DOMStorage` or evaluation in a frame of that origin.
- `state save <file>` writes JSON in a **Playwright-compatible
  `storageState` format** (cookies + localStorage per origin, plus our
  extension for sessionStorage and IndexedDB, opt-in with `--indexeddb`).
  Existing Playwright auth fixtures then work unchanged.
- `state load <file>`, and `open --state <file>`: apply the cookies before the
  first navigation. localStorage is seeded by an init script scoped to its
  origin, which then removes itself.
- Warn that state files hold secrets. Write them with mode 0600.
- `--profile <name>` (plan 02) is the heavier alternative for real persistent
  profiles.

## Milestone 7.5 — Extensions (stretch)

- `open --extension <dir>`: `--load-extension` +
  `--disable-extensions-except`. This requires `--headless` (new headless
  supports extensions). Test with a trivial fixture extension.

## Tests

- Routing: fulfill/abort/delay/modify/patch-json/times. Rules apply to an
  OOPIF and a popup opened afterwards, and to a request fired by an inline
  script on first load (proves pre-script application). There is also an
  overhead micro-benchmark and a service worker fixture.
- Emulation: each setting is verified from page JS (`matchMedia`,
  `navigator.language`, `Intl…timeZone`, `navigator.geolocation`,
  `navigator.onLine`, `devicePixelRatio`, `ontouchstart`).
- State round trip: log in on the fixture → `state save` → new session →
  `state load` → you are logged in. Compatibility test: load a state file
  produced by real Playwright (checked-in fixture).

## Acceptance criteria

- [ ] Every emulation and route setting is applied to new tabs before their
      first script runs.
- [ ] Playwright storageState files load unchanged.
- [ ] Route overhead < 1 ms/request p95 with 20 rules.

## Decisions log

_(append here)_
- 7.1: One `Fetch` pattern (`*`, request stage) per page and iframe session while any rule exists; the daemon matches rules itself. Our patterns (globs where `*` stops at `/`, `/regex/`, parts of URLs, as `wait --url` takes them) do not map onto Fetch's wildcards, and one pattern keeps rule changes to a single `Fetch.enable` per session. `--patch-json` asks for the response stage per request (`continueRequest` with `interceptResponse`), so other responses are never paused.
- 7.1: `route`'s JSON answer is `--json-body`, not the plan's `--json`: `--json` is the global output flag (plan 08 §4) on every command. `--delay` takes a duration with a unit (`2s`, `500ms`), like `--timeout`.
- 7.1: A rule has exactly one action; the CLI refuses two (e.g. `--abort` with `--json-body`). `--delay` combines with any action (a slow 500 is one rule); alone it delays and continues.
- 7.1: `block` takes patterns and the presets as flags (`block --images --fonts --media --ads`), one rule each, shown by their label in `routes`. Images, fonts and media go by resource type; `--ads` is a regular expression over 25 well-known ad and tracking hosts and their subdomains.
- 7.1: Rules live in the daemon's session settings, not the browser: they apply to every tab, popup and OOPIF (set up while the target is paused, so before its first script) and survive `open --restart`. They do not survive a daemon replaced by a CLI upgrade (the old daemon's rules are not handed over); a follow-up. The commands take no tab lock; `--tab` gets a warning, since rules are no tab's.
- 7.1: `Fetch` reports a `fetch()` as `XHR`, so for `--resource-type` `xhr` and `fetch` are one type; the journal shows Chrome's name (`xhr`).
- 7.1: While routes exist, requests skip service workers (`Network.setBypassServiceWorker`) and the HTTP cache (`Network.setCacheDisabled`), as Playwright does: a cache hit or a service worker's answer never reaches `Fetch`. `open --bypass-service-worker` is a session setting (kept like `--ignore-https-errors`) that skips service workers without routes too.
- 7.1: A fulfilled request with an `Origin` gets `Access-Control-Allow-Origin: <origin>` and `-Allow-Credentials: true` unless the rule sets its own, and a CORS preflight for a request a fulfilling rule matches is answered 204 allowing what it asks, so mocking another origin's API needs no CORS headers. Preflights are no hits.
- 7.1: `--times N` removes the rule after its N-th match; when the last rule goes, interception stops. Ids (`r1`) are never reused in a session.
- 7.1: The request journal (1,000 entries, `routes --log`) records every request routing saw, matched or not, with the rule and outcome (`fulfill 200`, `abort blockedbyclient`, `continue`, `patch json (not JSON; not patched)`, `preflight allowed`). Rules carry an optional browser context (unset by the CLI) and are added in bulk (`route` and `block` send a list), for plan 11's per-context mock sets.
- 7.1: `Fetch` events now come through a lossless stream (`Session::events_lossless`, an unbounded channel per listener) in their own task per session: a dropped `Fetch.requestPaused` would hang the page. `--http-credentials` shares that task and the same `Fetch.enable`.
- 7.1: Requests of dedicated and shared workers are not routed yet (their sessions get no `Fetch.enable`); service workers are bypassed instead. A follow-up.
- 7.1: The overhead benchmark is `overhead_with_20_rules` in `crates/engine/tests/route.rs` (`#[ignore]`; 300 sequential fetches with and without 20 non-matching rules, p95 difference). Not run: deferred, benchmarks run after all plans. The acceptance criterion stays unticked.
- 7.1: The first acceptance criterion stays unticked until 7.2's emulation settings are applied the same way; routes are (tested for a request fired by an inline script on first load, a popup, and an OOPIF).
