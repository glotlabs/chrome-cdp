<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp keyboard`

Hold keys down across commands (Shift while clicking, …)

```text
chrome-cdp keyboard [OPTIONS] <COMMAND>
```

```text
`keyboard down` presses a key and holds it; `keyboard up` releases it.
Held modifiers (Shift, Control, Alt, Meta, Mod) go with the tab's later
click, dblclick, press, type, mouse and drag events, as a user's held key
does. Keys stay held across navigations and are dropped when the tab
closes; `status` lists them. For a shortcut, `press Control+A` is simpler.

Examples:
  chrome-cdp keyboard down Shift
  chrome-cdp click e7                      # a Shift+click
  chrome-cdp click e9                      # extends the selection
  chrome-cdp keyboard up Shift
  chrome-cdp keyboard up --all
```

## `chrome-cdp keyboard down`

Press a key and hold it until `keyboard up`

```text
chrome-cdp keyboard down [OPTIONS] <KEY>
```

Arguments:

- `<KEY>`: The key: Shift, Control, Mod, Enter, a, …

```text
Name the key as `press` does: Shift, Control, Alt, Meta, Mod (Meta on
macOS, Control elsewhere), Enter, a, A, … A held key is not pressed again.

Examples:
  chrome-cdp keyboard down Shift
  chrome-cdp keyboard down Mod
  chrome-cdp keyboard down a               # a keydown the page sees, held
```

## `chrome-cdp keyboard up`

Release a held key, or all of them

```text
chrome-cdp keyboard up [OPTIONS] [KEY]
```

Arguments:

- `<KEY>`: The key to release

Options:

- `--all`: Release every held key, the last held first

```text
A key that is not held is a warning, not an error.

Examples:
  chrome-cdp keyboard up Shift
  chrome-cdp keyboard up --all
```

