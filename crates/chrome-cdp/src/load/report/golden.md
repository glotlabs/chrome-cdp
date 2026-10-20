# Load run 20261020-120000-ab12

- Profile e2e/load/shop.toml, open model
- Backend staging
- 40 iterations in 16s, 1 failed
- 41 arrivals, 1 dropped at max_users (100); at most 12 users at once
- 2 Chromes (chrome-headless-shell), up to 10 users each
- Client saturated for 6s of 16s (cpu first, at 0:08): the thresholds leave those seconds out; it held 8 users (4 per Chrome) before
- Thresholds: 1 of 2 passed

## Thresholds

| | Threshold | Measured | |
|---|---|---|---|
| ✓ | `p95(GET /api/products/:id) < 300ms` | 60 ms |  |
| ✗ | `p75(inp) < 200ms` | no value | inp had no value to measure |

## Load generator

Saturated: for 5s in a row, CPU above 90%, free memory below 10%, or timers late by more than 50 ms at p99 (`[load.capacity]`, on_saturation = "warn").

- Saturated 0:08 to 0:14, cpu 97%

To do: lower the rate to about 1.6/s, or run the profile on a bigger machine.

| | |
|---|---|
| CPU at p50 | 60% |
| CPU at p95 | 96% |
| CPU at most | 97% |
| Load average at most | 14.20 |
| Free memory at least | 41% |
| Timer lag at p99 | 3.2 ms |
| Timer lag at most | 61 ms |
| Pages' main-thread lag at p99 | 48 ms |
| Chromes' CPU at most | 13.4 cores |
| Chromes' memory at most | 3GB |
| chrome-cdp's CPU at most | 0.21 cores |
| chrome-cdp's memory at most | 60MB |

## Requests

400 requests (2.5/s), 0.2% errors; p50 12 ms, p95 24 ms, p99 36 ms.

| Endpoint | Count | /s | Errors | p50 | p90 | p95 | p99 | Max |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| `GET /api/products/:id` | 120 | 2.5 | 0.8% | 30 ms | 45 ms | 60 ms | 90 ms | 120 ms |
| `static js` | 80 | 2.5 | 1.2% | 4.0 ms | 6.0 ms | 8.0 ms | 12 ms | 16 ms |

## Journeys

| Journey | Iterations | Failed | p50 | p90 | p95 | p99 | Max |
|---|--:|--:|--:|--:|--:|--:|--:|
| browse \| buy (`e2e/shop.cdp:1`) | 40 | 1 | 1.50 s | 2.00 s | 2.10 s | 2.50 s | 3.00 s |

An iteration's time counts from when its user arrived.

## Steps

| Step | Command | Count | Errors | p50 | p95 | p99 | Max |
|---|---|--:|--:|--:|--:|--:|--:|
| `e2e/shop.cdp:3` | `goto /` | 40 | 1 | 210 ms | 420 ms | 630 ms | 840 ms |

## What users saw

Per page view; Web Vitals are rated at p75.

| | Page views | p50 | p75 | p95 | Max |
|---|--:|--:|--:|--:|--:|
| LCP | 80 | 400 ms | 480 ms | 800 ms | 1.60 s |
| CLS | 80 | 0.010 | 0.012 | 0.020 | 0.040 |

## Errors

| Kind | Count |
|---|--:|
| ExpectFailed | 1 |
| http_5xx | 1 |
| uncaught exceptions | 2 |

Uncaught exceptions:

- 2 × `TypeError: x is undefined`

## Compared with 20261019-090000-cd34

1 of 2 numbers changed by more than 10%.

| | Before | Now | Change |
|---|--:|--:|--:|
| `p95(GET /api/products/:id)` | 40 ms | 60 ms | +50% |

## Files

- summary.json: these numbers, as `--json` prints them
- metrics.jsonl: each second of each group
- report.html: this report, with charts over time
- e2e/.results/load/20261020-120000-ab12/failures: the page, console and requests of 1 failed iterations
