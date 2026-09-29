<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp pdf`

Print the page to a PDF file (headless browsers only)

```text
chrome-cdp pdf [OPTIONS]
```

Options:

- `-o, --output <PATH>`: Where to save it (`-` for stdout); a numbered file in the session's pdfs/ directory if not given
- `--format <PAPER>`: The paper size (one of `Letter`, `Legal`, `Tabloid`, `A3`, `A4`, `A5`; default `Letter`)
- `--landscape`: Turn the paper sideways
- `--background`: Print background colours and images
- `--margin <LENGTHS>`: Page margins: one to four lengths in CSS order (top, right, bottom, left), each in in, cm, mm or px: 1cm, 10mm,15mm. Default 1cm
- `--pages <RANGES>`: Print only these pages: numbers and ranges, comma-separated (1-3,5; 3- runs to the end). Default all
- `--css-page-size`: Use the page's own CSS `@page` size where it sets one, instead of --format

```text
Prints the file's path (-q: only the path); the PDF goes to the session's
pdfs/ directory (1.pdf, 2.pdf, …) unless -o names a file, or -o - for
stdout. Chrome prints the page as its print dialog would: with the print
media styles, on pages of --format paper. Chrome prints only when headless.
Needs a running session.

Examples:
  chrome-cdp pdf
  chrome-cdp pdf -o invoice.pdf --format A4 --background
  chrome-cdp pdf --landscape --margin 1cm
  chrome-cdp pdf --margin 10mm,15mm           # top and bottom, left and right
  chrome-cdp pdf --pages 1-3,5 --css-page-size
```

