<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp emulate`

Make pages look like a phone, tablet or desktop; list or reset

```text
chrome-cdp emulate [OPTIONS] [DEVICE]...
```

Arguments:

- `<DEVICE>`: A device name as `--list` shows it (case and spaces are loose), or `reset`. Without it, prints what the session emulates

Options:

- `--list`: List the devices

```text
A device sets the viewport, pixel ratio, mobile mode, touch and user agent
(the table comes from Playwright's device descriptors). Without DEVICE,
prints everything the session emulates; `emulate reset` turns all of it
off (viewport, user agent, locale, time zone, geolocation, media,
network, CPU and time). Emulation belongs to the session: it applies to
every tab, popup and frame, those opened later too, before the page's
first script runs.

Examples:
  chrome-cdp emulate 'iPhone 15'
  chrome-cdp emulate Pixel 7 landscape
  chrome-cdp emulate --list
  chrome-cdp emulate                      # what the session emulates
  chrome-cdp emulate reset
```

