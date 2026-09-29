<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp downloads`

List the files the session's pages downloaded

```text
chrome-cdp downloads [OPTIONS]
```

Options:

- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Downloads go to the session's downloads directory, named as the server
suggests (made unique). A command that starts one says so; `wait
--download` waits for it to finish.

Examples:
  chrome-cdp downloads
  chrome-cdp downloads -q         # one path per line
```

