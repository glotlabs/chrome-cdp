# Durations: the run order and shards by time

Each `chrome-cdp test` run keeps how long its tests took, and the next
run uses it to finish sooner: the longest tests start first, `--shard`
splits the suite by time rather than by count, and `--failed-first`
starts the tests that failed last time first.

## The durations file

`e2e/.durations.json` (beside the artifacts directory; `[e2e] durations`
names another file) has, for each test by backend, file and name:

- `ms`: a moving average of its passing durations. Each new passing run
  weighs a fifth once there are five, so a test that got slower shows it
  within a few runs, and one slow run does not move it much. A failed
  run leaves it as it was.
- `runs`: how many passing runs the average has (at most 5).
- `last`: how its last run ended, `passed`, `failed` or `flaky`.

```json
{
  "version": 1,
  "backends": {
    "mock": {
      "e2e/scenarios/checkout.cdp": {
        "places an order": { "ms": 1312, "runs": 5, "last": "passed" }
      }
    }
  }
}
```

Every run updates the tests it ran (not `--list`). A test that no longer
exists is dropped: one a file the run read no longer has, and every test
of a file that is gone. A file that does not parse is a warning; the run
then works as without one and writes a new file.

The file changes on every run, so it is not committed: `init e2e` adds it
to `.gitignore`. Cache it in CI instead (below).

```toml
[e2e]
durations = ".cache/e2e-durations.json"   # default: beside artifacts
```

## Longest first

With durations known, the workers take the tests longest first, so a
slow test does not start last and end the run late. A test the file has
no time for (a new one) goes first, as if it were long. Skipped tests go
last. Without the file, the order is the plan's: files by path, tests as
written.

A file's `@before-all` still runs once, before the first of its tests to
start (a worker that needs it meanwhile waits), and its `@after-all`
after its last, wherever they fall in the order.

A line prints as its test ends, so the lines come in the order the tests
finish. The closing summary, the failed tests the error names, `--json`
and the reporters keep the plan's order.

## Shards by time

`--shard 2/4` with durations known splits the selected tests so each
shard's total time is as equal as greedy packing gives: the longest test
first, each into the shard with the least time so far. A test the file
has no time for counts as the mean of those it has. The split depends
only on the selected tests and the file, so every CI machine that
restores the same file computes the same shards. Without the file (or
with no time for any selected test), the shards are near-equal parts of
the list, as before.

Every shard of a suite must start from the same file. A run updates it,
so on one machine, `--shard 1/2` then `--shard 2/2` with the file the
first left would split differently, and some tests would run twice and
others not at all: give each shard a copy of the same file (as CI
machines that restore the same cache have), or run them without one.

`test --list --shard 2/4` shows every shard's expected time:

```
$ chrome-cdp test --list --shard 2/4 --jobs 4
e2e/scenarios/cart.cdp:3 adds a product to the cart
…
251 tests
expected time by shard (from e2e/.durations.json; the tests' times added up, and about how long with 4 at once):
  1/4: 250 tests, 4m7s, about 1m1s
  2/4: 251 tests, 4m8s, about 1m2s (listed)
  3/4: 249 tests, 4m6s, about 1m1s
  4/4: 250 tests, 4m7s, about 1m1s
```

A test's time is what it took in the runs the file kept, and tests take
longer when more run at once on the machine, so "about" is a guide; the
split only needs the times in proportion.

`--json` has them in `data.shards`.

## Failed first

`chrome-cdp test --failed-first` starts the tests whose last run failed
(`"last": "failed"`) before the rest, longest first among them. On a red
build the failure shows in seconds; with `--bail`, the run stops there.

## In CI

The workflow `init e2e` writes restores the file before the tests and
saves it after, also when they fail:

```yaml
      - uses: actions/cache/restore@v4
        with:
          path: e2e/.durations.json
          key: e2e-durations-${{ github.run_id }}-${{ github.run_attempt }}-${{ strategy.job-index }}
          restore-keys: e2e-durations-
      - name: E2E tests
        run: chrome-cdp test --reporter text,junit --shard ${{ matrix.shard }}
      - uses: actions/cache/save@v4
        if: always()
        with:
          path: e2e/.durations.json
          key: e2e-durations-${{ github.run_id }}-${{ github.run_attempt }}-${{ strategy.job-index }}
```

Every shard restores the newest saved file, so the shards of a run agree
on the split. Each shard saves the file with its own tests' new times;
the other tests keep the times they had, so the file stays whole.
