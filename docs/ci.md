# CI

chrome-cdp is one static binary plus a Chrome, and every command exits
non-zero when it fails, so it runs in any CI. This page shows GitHub
Actions; the same steps work elsewhere, or use the
[Docker image](docker.md).

## E2E tests: let `init e2e` write the workflow

For a project with scenario files, `chrome-cdp init e2e` writes
`.github/workflows/e2e.yml`. It installs chrome-cdp, checks the scenario
and mock files (`fmt --check`, and `lint --format github`, which shows
problems inline on the pull request), runs `chrome-cdp test` in two shards
with JUnit output, keeps the tests' durations in the cache (so the next
run starts the longest first and splits the shards by time, see
[e2e/durations.md](e2e/durations.md)), and uploads `e2e/.results/` (the
failed tests' `report.md`, screenshots and logs) when a test fails. It also has a
commented-out nightly job that runs the same scenarios against a real
backend with `--verify-mocks`. [e2e/quickstart.md](e2e/quickstart.md#5-run-it-in-ci)
walks through it.

## A workflow from scratch

This one runs a smoke script against the app's dev server on Linux:

```yaml
name: Smoke
on: [push, pull_request]

jobs:
  smoke:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install chrome-cdp
        run: |
          curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh | sh
          echo "$HOME/.local/bin" >> "$GITHUB_PATH"

      # Chrome for Testing, kept between runs.
      - uses: actions/cache@v4
        with:
          path: ~/.cache/chrome-cdp
          key: chrome-cdp-${{ runner.os }}-${{ runner.arch }}
      - name: Install Chrome
        run: |
          chrome-cdp install --with-deps
          chrome-cdp doctor

      - name: Start the app
        run: |
          npm ci
          npm run dev &
          npx wait-on http://localhost:3000

      - name: Smoke test
        env:
          APP_PASSWORD: ${{ secrets.APP_PASSWORD }}
        run: chrome-cdp run e2e/smoke.cdp

      - name: Screenshot on failure
        if: failure()
        run: chrome-cdp screenshot --full -o smoke-failure.png
      - uses: actions/upload-artifact@v4
        if: failure()
        with:
          name: smoke-failure
          path: smoke-failure.png
```

Notes on each step:

- **Installing chrome-cdp.** The install script downloads the release
  binary for the runner (Linux or macOS, x86_64 or arm64) and checks its
  SHA-256; pin a version with `sh -s -- --version 1.2.3`. `npx chrome-cdp`
  and `cargo binstall chrome-cdp` work too ([install.md](install.md)).
- **Chrome.** `chrome-cdp install` downloads Chrome for Testing into
  `~/.cache/chrome-cdp`. On Linux, `--with-deps` also installs the system
  libraries it needs (with `sudo`, which GitHub's runners allow) and, on
  Ubuntu 23.10 and later, the AppArmor profile Chrome's sandbox needs
  there. Without an install step, chrome-cdp uses the runner's own Google
  Chrome if it has one, or installs Chrome for Testing on first use when
  `CHROME_CDP_AUTO_INSTALL=1` is set. `--version 154` pins a version, so
  a new Chrome does not change your results unannounced.
- **`doctor`** prints what chrome-cdp found (Chrome, libraries, sandbox,
  session dir) and fails the step with a fix if something is missing, so
  a broken runner shows up before the tests.
- **Secrets.** Pass them by environment variable and read them with
  `${env:APP_PASSWORD}` in scripts, never as literal arguments, which end
  up in logs. In scenario files, `${secret:NAME}` also masks the value in
  all output.
- **The session.** A job's commands share the `default` session: the
  first one starts its daemon and Chrome, later ones reuse them, and the
  runner stops them with the job (`chrome-cdp close` does it earlier).
  Give commands that run in parallel their own `--session` (or
  `CHROME_CDP_SESSION`).
- **Output.** Commands print plain text without colours when not on a
  terminal. `--json` is there for steps that parse results with `jq`.

## macOS runners

The same steps work on `macos-latest` (arm64) and `macos-15-intel`:
`--with-deps` only warns that it has nothing to do there, and the cache dir is
`~/Library/Caches/chrome-cdp`:

```yaml
      - uses: actions/cache@v4
        with:
          path: |
            ~/.cache/chrome-cdp
            ~/Library/Caches/chrome-cdp
          key: chrome-cdp-${{ runner.os }}-${{ runner.arch }}
```

## In a container

GitLab CI, Buildkite and friends can use the published image, which
already holds Chrome and the fonts:

```yaml
# .gitlab-ci.yml
smoke:
  image:
    name: ghcr.io/glotlabs/chrome-cdp
    entrypoint: [""]
  script:
    - chrome-cdp run e2e/smoke.cdp
```

[docker.md](docker.md) explains the image and Chrome's sandbox inside
containers.

## Keeping tests reliable

- Never `sleep`: actions and navigation wait by themselves, and `wait`
  and `expect` cover the rest ([waiting.md](waiting.md)).
- Pin the Chrome version, and make screenshot baselines on the same OS
  and fonts that check them (the Docker image gives both).
- `chrome-cdp test --repeat 20` runs each test 20 times without retries,
  to find a flaky test before CI does.
- `chrome-cdp test --reporter junit` writes a JUnit report next to the
  results (or `junit=FILE`), which CI systems show per test.
