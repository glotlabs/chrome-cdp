<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp think`

Pause a load run's user here, as a person would (a no-op outside `load`)

```text
chrome-cdp think [OPTIONS] [TIME]
```

Arguments:

- `<TIME>`: How long, e.g. 3s, or MIN..MAX for a time between, e.g. 2s..8s. Default: the load run's --think

```text
A step for `load` (docs/e2e/load.md): the virtual user waits that long
before its next step, `MIN..MAX` a time picked evenly between the two,
and with no time the load run's --think. Under `test`, `run`, `batch` and
on the command line it does nothing, so a scenario keeps running at full
speed as a test.

Examples:
  chrome-cdp think 3s
  chrome-cdp think 2s..8s
  chrome-cdp think
```

