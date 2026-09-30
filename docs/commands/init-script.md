<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp init-script`

Run scripts in every page before its own: add, list or remove

```text
chrome-cdp init-script [OPTIONS] <COMMAND>
```

```text
An init script runs in every new document of the browser context (tabs,
popups and frames, those opened later too) before the page's own scripts, in the
order they were added. Stub APIs, set feature flags, or record what the
page does. Init scripts belong to the browser context (the session's own,
unless --context); the session's own stay when the browser is restarted.
For the very first page, give them to open: open --init-script FILE URL.

Examples:
  chrome-cdp init-script add ./stubs.js
  chrome-cdp init-script add --code 'window.FEATURE_X = true' --now
  chrome-cdp init-script list
  chrome-cdp init-script remove s1
```

## `chrome-cdp init-script add`

Add a script that runs in every new document before its own

```text
chrome-cdp init-script add [OPTIONS] <FILE|--code <JS>>
```

Arguments:

- `<FILE>`: The script's file; `-` reads it from stdin

Options:

- `--code <JS>`: The script itself, instead of a file
- `--world <WORLD>`: Where it runs: the page's own world, or chrome-cdp's isolated one (one of `main`, `isolated`; default `main`)
- `--now`: Also run it in the documents open now

```text
Prints the script's id (s1, s2, …), its world and size. It runs from each
page's next load (`reload` for the current one), or at once in the open
pages too with --now. The script is checked in the active tab first; one
that does not compile is refused. Errors it throws name its file (or
init-script-s1.js) in `console` and `errors`.

--world main (the default) runs it with the page's globals, to stub or
wrap them; --world isolated runs it where `eval --world isolated` runs,
with the same DOM but out of the page's reach.

Examples:
  chrome-cdp init-script add ./stubs.js
  chrome-cdp init-script add --code 'localStorage.setItem("tour-done", "1")'
  chrome-cdp init-script add --code 'window.FEATURE_X = true' --now
  cat stubs.js | chrome-cdp init-script add -
```

## `chrome-cdp init-script list`

List the init scripts, in the order they run

```text
chrome-cdp init-script list [OPTIONS]
```

Options:

- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Each line reads: id, world, file (or the script's first line), size.

Examples:
  chrome-cdp init-script list
```

## `chrome-cdp init-script remove`

Remove init scripts from new documents

```text
chrome-cdp init-script remove [OPTIONS] <ID|--all>
```

Arguments:

- `<ID>`: The scripts' ids, as `init-script list` shows them (s1, s2, …)

Options:

- `--all`: Remove every init script

```text
Open pages keep what a script already did until they load again.

Examples:
  chrome-cdp init-script remove s1
  chrome-cdp init-script remove --all
```

