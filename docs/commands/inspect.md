<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp inspect`

Open DevTools for the tab, in its own window (headed browsers)

```text
chrome-cdp inspect [OPTIONS]
```

Options:

- `--panel <PANEL>`: The panel to open on (one of `elements`, `console`, `network`, `sources`, `application`, `performance`, `memory`, `lighthouse`, `security`, `recorder`)

```text
For a human pairing with the agent: DevTools opens next to the browser
window, on --panel if given. A headless browser has no window to open it
in: start one with open --headed --restart. Needs a running session.

Examples:
  chrome-cdp inspect
  chrome-cdp inspect --panel console
  chrome-cdp inspect --tab t2 --panel network
```

