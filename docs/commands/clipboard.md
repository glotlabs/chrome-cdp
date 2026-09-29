<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp clipboard`

Read or write the clipboard's text, through the page

```text
chrome-cdp clipboard [OPTIONS] <COMMAND>
```

```text
The page's clipboard API does it, as a page's own copy and paste buttons
would: the tab's page must be on https (or http on localhost), and gets
the clipboard permissions for the rest of the session. The clipboard is
the browser's (the system's when headed): what a page copied can be read,
and what is written pastes with press Mod+V.

Examples:
  chrome-cdp clipboard read
  chrome-cdp clipboard write 'hello'
  chrome-cdp press Mod+V --in e5
```

## `chrome-cdp clipboard read`

Print the text on the clipboard

```text
chrome-cdp clipboard read [OPTIONS]
```

Options:

- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Examples:
  chrome-cdp clipboard read
  chrome-cdp click e7 && chrome-cdp clipboard read -q   # what a Copy button copied
```

## `chrome-cdp clipboard write`

Put text on the clipboard

```text
chrome-cdp clipboard write [OPTIONS] <TEXT>
```

Arguments:

- `<TEXT>`: The text; `-` reads it from stdin

```text
Examples:
  chrome-cdp clipboard write 'hello'
  chrome-cdp clipboard write - < notes.txt
```

