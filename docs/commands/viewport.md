<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp viewport`

Set the viewport: WIDTHxHEIGHT, pixel ratio, mobile, touch

```text
chrome-cdp viewport [OPTIONS] <SIZE>
```

Arguments:

- `<SIZE>`: WIDTHxHEIGHT in CSS pixels (e.g. 1440x900), or `reset`

Options:

- `--dpr <N>`: The device pixel ratio (devicePixelRatio), e.g. 2 or 3
- `--mobile`: A mobile browser: the page's meta viewport counts, scrollbars overlay
- `--touch`: Touch events, and navigator.maxTouchPoints &gt; 0 (for `tap`)

```text
In headless mode pages get 1280x800 until a viewport is set (unless
--chrome-arg=--window-size sets the window); headed, the window decides.
`viewport reset` goes back to that. Applies to every tab of the session.

Examples:
  chrome-cdp viewport 1440x900
  chrome-cdp viewport 390x844 --dpr 3 --mobile --touch
  chrome-cdp viewport reset
```

