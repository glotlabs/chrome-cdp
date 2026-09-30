<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp why`

Explain why an action on an element would wait or fail, check by check, without acting

```text
chrome-cdp why [OPTIONS] <LOCATOR>
```

Arguments:

- `<LOCATOR>`: The element: a ref (e12) or a locator that matches one

Options:

- `--for <ACTION>`: The action whose checks run (one of `click`, `hover`, `fill`, `type`, `check`, `select`, `upload`, `drag`; default `click`)
- `--no-scroll`: Leave the page where it is: an element out of view is not hit-tested
- `--strict-refs`: A ref whose element is gone fails, instead of being re-resolved to the one element with its role and name (env `CHROME_CDP_STRICT_REFS`)

```text
Runs the checks the action needs (--for, default click) once, now, and
prints each with its cause: resolve (no match, or which elements an
ambiguous locator matches), visible (display: none on which ancestor,
visibility, opacity: 0, an empty box), stable (how far it moved between
two frames, and the animations on it), enabled (disabled, a disabled
fieldset, aria-disabled, inert, a modal dialog open elsewhere), editable
(fill, type), in view (which container scrolled, or what clips it) and
hit test (what covers it and the button that likely dismisses it, or
pointer-events: none). The last line says what the action would do:
act now, wait until the timeout, or fail at once. Elements are named as
the snapshot names them, with refs. It scrolls the element into view as
the action would; --no-scroll leaves the page where it is and skips the
hit test when the element is out of view. It does not act, wait or
retry, and exits 0 whatever it finds. -q prints the last line. Needs a
running session.

Examples:
  chrome-cdp why e12
  chrome-cdp why '#email' --for fill
  chrome-cdp why 'role=button[name="Pay"]' --no-scroll
```

