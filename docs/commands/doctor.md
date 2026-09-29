<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp doctor`

Check the setup: Chrome, its libraries and sandbox, the session dir, macOS quarantine and version skew; prints a fix for each problem

```text
chrome-cdp doctor [OPTIONS]
```

Options:

- `--chrome <PATH>`: The Chrome or Chromium binary to check. Default: the one `open` would launch
- `--no-launch`: Do not start Chrome; only look at the files and the system

```text
Each check prints ok, warn, FAIL or skip, and a fix for anything not ok.
Unless --no-launch is given, doctor starts the Chrome it finds (headless,
a temporary profile) and closes it again: the one sure test of its
libraries and sandbox. It exits 4 when a check fails, 0 otherwise.

Examples:
  chrome-cdp doctor
  chrome-cdp doctor --no-launch                 # only look, start nothing
  chrome-cdp doctor --chrome /usr/bin/chromium
  chrome-cdp doctor --json                      # every check, with its fix
```

