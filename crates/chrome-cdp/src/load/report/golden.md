# Load run 20261020-120000-ab12

- Profile e2e/load/shop.toml, open model
- Backend staging
- 40 iterations in 16s, 1 failed
- 41 arrivals, 1 dropped at max_users (100); at most 12 users at once
- 2 Chromes (chrome-headless-shell), up to 10 users each
- Thresholds: 1 of 2 passed

## Thresholds

| | Threshold | Measured | |
|---|---|---|---|
| ✓ | `p95(GET /api/products/:id) < 300ms` | 60 ms |  |
| ✗ | `p75(inp) < 200ms` | no value | inp had no value to measure |

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
