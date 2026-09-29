<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp media`

Emulate CSS media: print, dark mode, reduced motion, forced colors

```text
chrome-cdp media [OPTIONS] [reset]
```

Arguments:

- `<reset>`: `reset`: every media feature back to the browser's own (one of `reset`)

Options:

- `--type <TYPE>`: The media type (one of `screen`, `print`)
- `--color-scheme <SCHEME>`: prefers-color-scheme (one of `light`, `dark`)
- `--reduced-motion <VALUE>`: prefers-reduced-motion (one of `reduce`, `no-preference`)
- `--forced-colors <VALUE>`: forced-colors (Windows high contrast) (one of `active`, `none`)

```text
Each flag sets one media feature and keeps the others; `media reset` gives
all of them back to the browser.

Examples:
  chrome-cdp media --color-scheme dark
  chrome-cdp media --reduced-motion reduce --forced-colors active
  chrome-cdp media --type print
  chrome-cdp media reset
```

