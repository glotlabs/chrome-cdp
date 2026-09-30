<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp clipboard`

Read or write the clipboard (text, HTML or a PNG), through the page

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
  chrome-cdp clipboard write --html '<b>Bold</b> move'
  chrome-cdp clipboard read --type image/png -o copied.png
  chrome-cdp press Mod+V --in e5
```

## `chrome-cdp clipboard read`

Print the text on the clipboard, its HTML, or save its image

```text
chrome-cdp clipboard read [OPTIONS]
```

Options:

- `--type <TYPE>`: The type to read (one of `text/plain`, `text/html`, `image/png`; default `text/plain`)
- `-o, --output <PATH>`: Where to save an image (`-` for stdout); needed with `--type image/png`
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
A type the clipboard does not hold is an error that lists the types it
has. An image needs -o: a file, or - for stdout.

Examples:
  chrome-cdp clipboard read
  chrome-cdp click e7 && chrome-cdp clipboard read -q   # what a Copy button copied
  chrome-cdp clipboard read --type text/html
  chrome-cdp clipboard read --type image/png -o copied.png
```

## `chrome-cdp clipboard write`

Put text, HTML or a PNG image on the clipboard

```text
chrome-cdp clipboard write [OPTIONS] [TEXT]
```

Arguments:

- `<TEXT>`: The text; `-` reads it from stdin

Options:

- `--html <HTML>`: HTML to put on the clipboard as text/html; `-` reads it from stdin
- `--file <IMAGE.png>`: A PNG image to put on the clipboard as image/png

```text
With --html or --file, the clipboard gets one item with every part given;
--html without TEXT adds the HTML's text as text/plain, so a plain field
pastes it too.

Examples:
  chrome-cdp clipboard write 'hello'
  chrome-cdp clipboard write - < notes.txt
  chrome-cdp clipboard write --html '<p>Hello <b>world</b></p>'
  chrome-cdp clipboard write --file chart.png
```

