
## Flaky

3 of 20 runs failed at scenarios/flaky/settle.cdp:9:3 (`expect role=status text 'Paid $5' --timeout <t>`); the others passed.

Likely cause: **acted before the page settled** (confidence 0.90)

- `GET /e2e/api/flaky/price` was still open when `click '#pay'` (scenarios/flaky/settle.cdp:8:3) acted, in all 3 failing runs; it was answered up to <t> later
- in all 17 passing runs it was answered before the step acted (<t> before, at the closest)
- `click '#load'` (scenarios/flaky/settle.cdp:7:3) sent it

Fix: add an `expect` of what its answer shows before scenarios/flaky/settle.cdp:8:3, or `wait --response '**/e2e/api/flaky/price'` after scenarios/flaky/settle.cdp:7:3 (which sends it); `settle = "full"` in [e2e] waits for every request after each step, at a cost in time

Failed: run 1, run 2, run 15. Passed: run 3, run 4, run 5, run 6, run 7, run 8, run 9, run 10, run 11, run 12, … (17 in all).
Each run's steps.jsonl and requests.txt (with when each request was sent and answered) are in its folder: compare <root>/e2e/.results/scenarios/flaky/settle.cdp/pays-the-price/run-1 with <root>/e2e/.results/scenarios/flaky/settle.cdp/pays-the-price/run-3.
