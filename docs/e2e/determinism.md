# Determinism: the same result in every run

Flaky suites are the main reason teams give up on E2E tests. `chrome-cdp
test` removes the usual sources of flakiness before a test's first step:
every test's browser context gets the settings below, unless
`[e2e.determinism]` turns them off.

| What | By default | `[e2e.determinism]` |
|---|---|---|
| Time: `Date`, `Intl.DateTimeFormat`, and the mocks' `{{now}}` | frozen at 2026-01-01T10:00:00Z | `freeze_time = "…"`, or `false` |
| `Math.random`, `crypto.getRandomValues`, `crypto.randomUUID` | seeded (seed 0): the same values in every run and every page load | `random_seed = 42`, or `false` |
| CSS animations and transitions | animations end at once (in their final state, still firing `animationend`), no transitions, no caret, no smooth scrolling; `prefers-reduced-motion: reduce` | `disable_animations = false` |
| Locale | `en-US` (`navigator.language`, `Intl`, `Accept-Language`) | `locale = "de-DE"`, or `false` |
| Time zone | `UTC` | `timezone = "Europe/Oslo"`, or `false` |
| Viewport and pixel ratio | `viewport` of `[e2e]` (1280x800), pixel ratio 1 | `viewport`; `@viewport` or `@device` in a scenario |

```toml
[e2e.determinism]
freeze_time = "2026-01-01T10:00:00Z"
random_seed = 42
disable_animations = true
locale = "en-US"
timezone = "UTC"
```

Mock sets' random values (`{{uuid}}`, `{{faker.*}}`, a `latency` range)
use the same `random_seed`, and start over in every test, so a rerun gets
the same answers after the same delays.

## Moving the time on

Timers keep running in real time, so a page never stalls. `time advance`
moves the frozen time on, moves `performance.now()` on by as much, and
runs the `setTimeout` and `setInterval` callbacks due by then, each once,
in the order they are due. Expiry, polling and debounce can be tested
without waiting:

```
@test "the session expires after five minutes"
  goto /account
  time advance 5m
  expect text="Your session has expired" visible
```

For a countdown or an animation, `time pause` before `goto` stops its
timers and animation frames, and `time run-for 1s` fires the ones due in
that second, each interval tick and a frame every 16ms:

```
@test "the offer ends after the countdown"
  time pause
  goto /offer
  time run-for 10s
  expect text="The offer has ended" visible
```

A step can change any of these for its test: `time freeze
2026-12-24T18:00:00Z`, `time reset`, `locale de-DE`, `tz Asia/Tokyo`,
`media --reduced-motion no-preference`.

## How long a step waits

On the command line, `goto` waits after the load event until the network
has been idle for 500 ms and the DOM unchanged for 300 ms, and every
action for 150 ms of quiet: whoever reads the page next reads it once.
A test does not read once. Its `expect`s retry until they pass, and its
actions wait until their element can be acted on, so those waits only
add time (about 0.8 s for every `goto` of a page that is already done).
A test run settles for less:

- a navigation (a `goto`, or one a click caused) waits for the load
  event, then until the requests the page started since are done and
  the DOM went 50 ms without a change, 1 s at most;
- an action that did not navigate waits for its own requests and 50 ms
  of quiet.

```toml
[e2e]
settle = "test"   # the default; "full" waits as the command line does
```

`settle = "full"` is for apps that change the page late without
anything an `expect` can wait on: a value filled in by a request that
ends after the step, with no text or element that shows it arrived.
Most tests are better off waiting for what they need, which also says
why: `expect text="Paid" visible`, or `wait --response '**/api/price'`.
A step can still ask for the full wait itself: `goto /page --wait
settle`, `wait --settle`, or `wait --idle`.

The check for console errors at the end of a test still sees an error
that comes late: it first waits for the context's requests to end (1 s
at most).

## Finding flaky tests

```
chrome-cdp test e2e/scenarios/cart.cdp --repeat 20
```

runs each test 20 times (spread over `--jobs`), without retries, since
retries would hide what `--repeat` looks for. A test fails if one of its
runs fails; its line says how many did, then the likely cause, then the
first failure:

```
✗ e2e/scenarios/cart.cdp:6 adds an item (failed 3 of 20 runs, slowest 4.6s)
    flaky: 3 of 20 runs failed at e2e/scenarios/cart.cdp:9:3 (`expect role=status text 'Paid $5'`)
    likely cause: acted before the page settled (confidence 0.90)
      `GET /api/price` was still open when `click '#pay'` (e2e/scenarios/cart.cdp:8:3) acted, in all 3 failing runs; it was answered up to 691ms later
      in all 17 passing runs it was answered before the step acted (627ms before, at the closest)
      `click '#load'` (e2e/scenarios/cart.cdp:7:3) sent it
      fix: add an `expect` of what its answer shows before e2e/scenarios/cart.cdp:8:3, or `wait --response '**/api/price'` after e2e/scenarios/cart.cdp:7:3 (which sends it); `settle = "full"` in [e2e] waits for every request after each step, at a cost in time
    first failure at e2e/scenarios/cart.cdp:9:3: expect role=status text 'Paid $5'
```

Run it before committing a new test.

### Why a test is flaky

Every run keeps its own folder (`run-1/`, `run-2/`, …; a retry's
`try-2/`, …) with its `steps.jsonl` and its `requests.txt`, which says
when each request was sent and answered, from the test's start. When
some runs pass and some fail (or a retry passes), the runner takes the
step most failing runs failed at and compares the failing runs with the
passing ones up to it:

| Cause | What the runs showed |
|---|---|
| acted before the page settled (`unsettled`) | A request sent before a step was still open when the step acted in failing runs, and answered before it in every passing run. Names the request, and the step that sent it. The fix asks for an `expect` of what the answer shows (or a `wait --response`), and mentions `settle = "full"` last. |
| requests raced (`race`) | Two API requests were answered in one order in failing runs and in the other in every passing run. Names both. |
| moving target (`moving-target`) | The failing step's element did not stop moving, was covered, or kept being replaced. Names the covering element. |
| different requests (`different-requests`) | Failing runs sent a request no passing run did (or skipped one every passing run sent), or one no mock answered. |
| time or randomness (`time-or-randomness`) | `freeze_time` or `random_seed` is off, and the failing assertion saw another value in each failing run. |
| shared state (`shared-state`) | Against a real backend: the runs passed until one failed, and every run after it failed too. |
| unknown | None of the above: the first step whose result differs between a passing and a failing run. |

Several can fit; they are ranked by how many failing runs they explain,
and the confidence (0 to 1) is that share, lowered for the rules that
guess more. The text reporter prints the first under the test's line;
the failure's `report.md` gets a "Flaky" section with the others and the
runs to compare; `--json` adds `flake: {cause, summary, confidence,
evidence, hint, …}` to the test's result. The browser's own favicon
request is left out, since it comes in some runs only.

## Fonts and screenshot baselines

Fonts render differently on each OS, so `expect-screenshot` keeps a
baseline per OS (`home-linux.png`, `home-macos.png`). When a baseline
exists only for another OS, the test gets a warning and a baseline of
its own (in CI, where no baseline is made, it fails and says why):

```
✓ e2e/scenarios/home.cdp:3 looks right (1.4s)
    warning: the baseline home was made on macos only; fonts render differently on each OS, so linux needs one of its own …
```

For baselines that match on every machine, run the tests in one image
everywhere, on laptops and in CI. [`Dockerfile`](Dockerfile) builds one:
Node for the web servers, a fixed set of fonts, and a pinned Chrome for
Testing.

```
docker build -f docs/e2e/Dockerfile -t chrome-cdp-e2e .      # in the chrome-cdp checkout
docker run --rm -v "$PWD":/app -w /app chrome-cdp-e2e test   # in your app
docker run --rm -v "$PWD":/app -w /app chrome-cdp-e2e test --update-snapshots
```

The container runs as root, so Chrome runs without its sandbox there
(`chrome-cdp` warns about it); that is fine for tests of your own app.
