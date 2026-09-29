<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp perf`

Show how fast the page is: load timing, Web Vitals, blocking time, memory

```text
chrome-cdp perf [OPTIONS]
```

```text
One compact block: TTFB, FCP, DOMContentLoaded and load (ms after the
navigation started), the requests and bytes, LCP (and its element), CLS,
INP (once the page had an interaction, and which one was slowest), total
blocking time from long tasks, and the element count, listeners and JS
heap. LCP, CLS and INP are rated good, needs improvement or poor by the
Web Vitals thresholds. Interact first to measure INP. -q prints the LCP.
Needs a running session.

Examples:
  chrome-cdp perf
  chrome-cdp click e5 && chrome-cdp perf       # INP of that click
  chrome-cdp perf --json | jq .lcp
```

