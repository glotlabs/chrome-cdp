<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp errors`

Show the page's errors: uncaught exceptions, console.error and failed loads from its own origin

```text
chrome-cdp errors [OPTIONS]
```

Options:

- `--since <WHEN>`: Where to start: `last` (what this command has not shown yet), an entry number such as 12 (0 for all), a duration such as 30s (logged in the last 30 seconds), or a mark (see `mark`) (default `last`)
- `--grep <REGEX>`: Only entries whose text or URL matches this regular expression
- `--limit <N>`: Show at most this many entries, the newest (0 = all) (default `50`)
- `-f, --follow`: Keep printing new entries as they come, until Ctrl-C (or --timeout, if given)
- `--sourcemaps`: Resolve stack frames with the scripts' source maps (always on for localhost pages)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
The first thing to run when an action did not do what you expected. Like
`console`, it shows only what it has not shown before; `--since 0` shows
every error the tab kept. Stack frames of localhost pages are resolved with
source maps, so they point at your original source files.

Examples:
  chrome-cdp errors
  chrome-cdp errors --since 0 --limit 5
  chrome-cdp errors --sourcemaps --tab t2
```

