<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp frames`

List the tab's frames and workers as a tree

```text
chrome-cdp frames [OPTIONS]
```

Options:

- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
One line per frame, indented under its parent, in the order of the <iframe>
elements: `frame main URL` for the page, then `iframe "name" URL`, marked
`(out of process)` for a cross-site iframe that runs in its own process.
Each frame's dedicated workers follow its iframes; the shared and service
workers of the origins the tab's frames are on come last. --frame and
frame= take a frame's name, a part of its URL, or its id (in --json, or
one per line with -q).

Examples:
  chrome-cdp frames
  chrome-cdp frames --json | jq -r '.data.frames[].url'
  chrome-cdp eval --frame "$(chrome-cdp frames -q | sed -n 2p)" 'document.title'
```

