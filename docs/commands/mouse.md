<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp mouse`

Low-level mouse events at viewport coordinates

```text
chrome-cdp mouse [OPTIONS] <COMMAND>
```

```text
An escape hatch for what the element commands cannot express: canvas
drawing, drags in hops, a press held across commands. Coordinates are
CSS pixels from the top-left corner of the tab's viewport. The session
keeps where the mouse is and which buttons it holds between commands.

Examples:
  chrome-cdp mouse move 100 200
  chrome-cdp mouse down
  chrome-cdp mouse move 300 200 --steps 10
  chrome-cdp mouse up
  chrome-cdp mouse wheel 0 400
```

## `chrome-cdp mouse move`

Move the mouse to a point

```text
chrome-cdp mouse move [OPTIONS] <X> <Y>
```

Arguments:

- `<X>`: CSS pixels from the viewport's left edge
- `<Y>`: CSS pixels from the viewport's top edge

Options:

- `--steps <N>`: How many moves to get there (default `1`)

```text
From where the mouse is, in --steps even moves; buttons held stay held
(a drag).

Examples:
  chrome-cdp mouse move 100 200
  chrome-cdp mouse move 400 200 --steps 20
```

## `chrome-cdp mouse down`

Press a mouse button (where the mouse is, or at x y)

```text
chrome-cdp mouse down [OPTIONS] [X] [Y]
```

Arguments:

- `<X>`: Move here first: CSS pixels from the viewport's left edge
- `<Y>`: And from its top edge

Options:

- `--button <BUTTON>`: The mouse button: left for down; for up, the one held (one of `left`, `right`, `middle`)
- `--count <N>`: The click count the event carries (2 for the second press of a double click) (default `1`)

```text
The button stays held until `mouse up`, across commands.

Examples:
  chrome-cdp mouse down
  chrome-cdp mouse down 100 200 --button right
```

## `chrome-cdp mouse up`

Release a mouse button (where the mouse is, or at x y)

```text
chrome-cdp mouse up [OPTIONS] [X] [Y]
```

Arguments:

- `<X>`: Move here first: CSS pixels from the viewport's left edge
- `<Y>`: And from its top edge

Options:

- `--button <BUTTON>`: The mouse button: left for down; for up, the one held (one of `left`, `right`, `middle`)
- `--count <N>`: The click count the event carries (2 for the second press of a double click) (default `1`)

```text
Examples:
  chrome-cdp mouse up
  chrome-cdp mouse up 300 200
```

## `chrome-cdp mouse wheel`

Turn the mouse wheel where the mouse is

```text
chrome-cdp mouse wheel [OPTIONS] <DX> <DY>
```

Arguments:

- `<DX>`: Horizontal delta in CSS pixels
- `<DY>`: Vertical delta in CSS pixels

```text
DX and DY are CSS pixels: positive scrolls right and down. Use `scroll`
to scroll an element and see where it ended.

Examples:
  chrome-cdp mouse wheel 0 400
  chrome-cdp mouse wheel 0 -400
```

