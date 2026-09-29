<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp screenshot`

Save an image of the page, the whole page or one element

```text
chrome-cdp screenshot [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: Capture only this element: a ref (e12) or a locator that matches one

Options:

- `--full`: The whole page, not only what the viewport shows
- `--format <FORMAT>`: The image format; else the -o file's extension, else png (one of `png`, `jpeg`, `webp`)
- `--quality <N>`: Quality from 0 to 100, for jpeg and webp
- `-o, --output <PATH>`: Where to save it (`-` for stdout); a numbered file in the session's shots/ directory if not given
- `--scale <SCALE>`: css: one image pixel per CSS pixel; device: as many as the screen has (the device pixel ratio) (one of `css`, `device`; default `css`)
- `--max-size <PX>`: The longest side in pixels; a larger image is scaled down to fit (0 = no limit) (default `1568`)
- `--omit-background`: A transparent background where the page has none (png and webp)
- `--mask <LOCATOR>`: Paint over the elements this locator matches (repeatable)
- `--annotate`: Label the elements `snapshot -i` gives refs with their refs
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Prints the file's path (-q: only the path); the image goes to the
session's shots/ directory (1.png, 2.png, …) unless -o names a file, or
-o - for stdout. The format follows -o's extension unless --format says.

Images are in CSS pixels (--scale css) and at most --max-size pixels on
their longest side (scaled down to fit; 0 for no limit), so a point in the
image is a point on the page. An element is scrolled into view (and waited
for) first, and the image is its box. --annotate labels the elements
`snapshot -i` gives refs with those refs; --mask paints over elements.
Both are drawn only for the screenshot. Needs a running session.

Examples:
  chrome-cdp screenshot
  chrome-cdp screenshot --annotate            # label the refs in the image
  chrome-cdp screenshot e12                   # one element
  chrome-cdp screenshot --full -o page.png    # the whole page
  chrome-cdp screenshot --mask '.price' --mask e7
  chrome-cdp screenshot --format jpeg --quality 70 -o - > shot.jpg
```

