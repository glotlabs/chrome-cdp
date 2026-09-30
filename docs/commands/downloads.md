<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp downloads`

List the files the session's pages downloaded

```text
chrome-cdp downloads [OPTIONS]
```

Options:

- `--clear`: Delete the files of finished downloads and forget them
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Downloads go to the session's downloads directory (`open --downloads-dir`
picks another), named as the server suggests (made unique). A command that
starts one says so; `wait --download` waits for it to finish. --clear
deletes the files of finished downloads and empties the list; downloads
still in progress are kept, and no other file is touched. The list is the
daemon's: after an upgrade replaced it, it starts again (and says so).

Examples:
  chrome-cdp downloads
  chrome-cdp downloads -q         # one path per line
  chrome-cdp downloads --clear
```

