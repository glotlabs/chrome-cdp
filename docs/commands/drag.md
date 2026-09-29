<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp drag`

Drag an element onto another: mouse events, or HTML5 drag and drop

```text
chrome-cdp drag [OPTIONS] <FROM> <TO>
```

Arguments:

- `<FROM>`: The element to drag: a ref (e12) or a locator that matches one
- `<TO>`: The element to drop it on

Options:

- `--steps <N>`: How many mouse moves from one to the other (default `10`)
- `--force`: Skip the checks (visible, stable, not covered) on both
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Presses the left button on the first element, moves to the second in
--steps moves and releases there, which is what sortable lists built on
mouse or pointer events need. When the page starts an HTML5 drag
(draggable elements), it is finished with real drag events (dragenter,
dragover, drop) carrying the page's own drag data. Both elements are
waited for like hover. Needs a running session.

Examples:
  chrome-cdp drag e4 e9
  chrome-cdp drag 'text=Card 1' 'role=list[name="Done"]'
  chrome-cdp drag e4 e9 --steps 30          # slower, for fussy libraries
```

