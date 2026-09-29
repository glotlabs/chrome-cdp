<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp connect`

Attach the session to a browser that is already running

```text
chrome-cdp connect [OPTIONS] <ENDPOINT>
```

Arguments:

- `<ENDPOINT>`: A port (9222), host:port, http://host:port, or a ws:// DevTools URL

Options:

- `--idle-timeout <DURATION>`: Close the browser and stop the daemon after this long without commands, e.g. 10m or 2h; 0 means never. Default 30m (env `CHROME_CDP_IDLE_TIMEOUT`)

```text
The browser must run with --remote-debugging-port. `close` then only
detaches; the browser keeps running.

Examples:
  chrome-cdp connect 9222
  chrome-cdp connect http://127.0.0.1:9222 --session mine
  chrome-cdp connect ws://127.0.0.1:9222/devtools/browser/<id>
```

