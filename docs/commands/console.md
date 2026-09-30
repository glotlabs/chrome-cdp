<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp console`

Show what the page logged: console calls, uncaught exceptions and the browser's messages

```text
chrome-cdp console [OPTIONS]
```

Options:

- `--level <LEVEL>`: The least severe level shown (one of `error`, `warn`, `info`, `debug`; default `info`)
- `--since <WHEN>`: Where to start: `last` (what this command has not shown yet), an entry number such as 12 (0 for all), a duration such as 30s (logged in the last 30 seconds), or a mark (see `mark`) (default `last`)
- `--grep <REGEX>`: Only entries whose text or URL matches this regular expression
- `--limit <N>`: Show at most this many entries, the newest (0 = all) (default `50`)
- `-f, --follow`: Keep printing new entries as they come, until Ctrl-C (or --timeout, if given)
- `--sourcemaps`: Resolve stack frames with the scripts' source maps (always on for localhost pages)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Every tab records its console from the moment it opens (the newest 1,000
entries; see `open --console-buffer`). By default only entries this command
has not shown before are printed, so running it again shows what is new.
Stack frames of localhost pages are resolved with source maps, and shown
as files of your project when they are there (src/cart.ts:12, relative to
the directory with chrome-cdp.toml or .git, or `source_root` in the
config; `chrome-cdp help config`). Workers log here too, marked `[dedicated worker w.js]`; a shared or service worker's
messages are in every tab with a frame on its origin.

Examples:
  chrome-cdp console
  chrome-cdp console --level warn --since 0      # every warning and error so far
  chrome-cdp console --since 30s --grep 'fetch|api'
  chrome-cdp console --follow --timeout 1m       # keep printing new entries
```

