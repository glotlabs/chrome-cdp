<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp throttle`

Slow the network: a preset, or latency and bandwidth

```text
chrome-cdp throttle [OPTIONS] [PRESET]
```

Arguments:

- `<PRESET>`: slow-3g, fast-3g, 4g, or `off`

Options:

- `--latency <DURATION>`: Added before each response starts, e.g. 300ms
- `--down <KBPS>`: Download bandwidth in kbit/s
- `--up <KBPS>`: Upload bandwidth in kbit/s

```text
Presets (as in Chrome DevTools): slow-3g (2s latency, 400kbit/s both
ways), fast-3g (563ms, 1440kbit/s down, 675kbit/s up), 4g (165ms,
8100kbit/s down, 1350kbit/s up). Flags change a preset's values, or set
their own.

Examples:
  chrome-cdp throttle slow-3g
  chrome-cdp throttle --latency 300ms --down 1000 --up 500
  chrome-cdp throttle off
```

