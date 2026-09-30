<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp layout`

Lint what the page looks like: covered, overlapping, clipped, off-screen and too small elements, sideways scrolling, low contrast

```text
chrome-cdp layout [OPTIONS] [LOCATOR]
```

Arguments:

- `<LOCATOR>`: Check only inside this element: a ref (e12) or a locator that matches one

Options:

- `--viewports <LIST>`: Check at each of these viewports: mobile, tablet, desktop or WxH, comma-separated
- `--only <KINDS>`: Check only these kinds, comma-separated: covered, overlap, clipped, truncated, overflow-x, offscreen, small-target, contrast
- `--full`: Look for covered elements and low contrast down the whole page, a viewport at a time, not only in view
- `--min-target <PX>`: The least width and height of a target, in CSS pixels (default 24)
- `--stretch-text <PCT>`: Check again with every text this many percent longer, and report what only that finds
- `--all`: List info findings too (text truncated on purpose)
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)
- `--max-chars <N>`: Bound the output to this many characters (0 = no limit) (default `20000`)

```text
Reads geometry and computed styles in the page and its iframes (or
inside the element LOCATOR names) and prints one line per finding,
naming elements as the snapshot does, with refs:
  covered       an interactive element whose centre something else
                covers (in view; --full scrolls down the whole page)
  overlap       two interactive elements whose boxes overlap
  clipped       text cut off by overflow: hidden or clip
  truncated     text cut on purpose, by an ellipsis or a line clamp
                (info: listed with --all)
  overflow-x    the page scrolls sideways, and what sticks out
  offscreen     an interactive element no scroll can reach
  small-target  an interactive element under 24x24 px (WCAG 2.5.8; a
                link in a line of text, or one with room around it, is
                fine; --min-target changes 24)
  contrast      text below 4.5:1 (3:1 when large) against the pixels
                behind it, so text on images and gradients counts
--only takes some of these. --viewports mobile,tablet,desktop (or sizes,
390x844) checks at each size (mobile and tablet with touch), waiting for
the page to settle each time, and gives the tab its viewport back after.
--stretch-text 40 checks again with every text 40% longer, as a
translation might make it, and reports what only that finds. The page,
its scroll and its text are left as they were (should the session die
mid-run, a reload brings the text back). It exits 0 whatever it
finds; expect --layout fails on new findings. -q prints the number of
errors. Needs a running session.

Examples:
  chrome-cdp layout
  chrome-cdp layout --only covered,clipped --all
  chrome-cdp layout --viewports mobile,tablet,desktop
  chrome-cdp layout 'role=main' --stretch-text 40
  chrome-cdp layout --full --min-target 44
```

