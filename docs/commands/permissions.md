<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp permissions`

List, grant, deny or reset permissions (geolocation, camera, …)

```text
chrome-cdp permissions [OPTIONS] [COMMAND]
```

```text
Without a subcommand, lists the permissions the session set (`geo` and
`clipboard` grant some too). Permissions belong to a browser context, so
they apply to every tab of the session, or with --context to every tab of
that context only; without --origin, to every origin. For camera and
microphone streams, start the browser with `open --fake-media`.

Examples:
  chrome-cdp permissions
  chrome-cdp permissions grant notifications
  chrome-cdp permissions grant camera microphone --origin http://localhost:3000
  chrome-cdp permissions deny geolocation
  chrome-cdp permissions reset
```

## `chrome-cdp permissions grant`

Grant permissions, to every origin or to --origin

```text
chrome-cdp permissions grant [OPTIONS] <NAME>...
```

Arguments:

- `<NAME>`: Permission names, as the Permissions API has them

Options:

- `--origin <ORIGIN>`: Only for this origin (e.g. https://example.com)

```text
Names: geolocation, notifications, clipboard-read, clipboard-write, camera,
microphone, midi, midi-sysex, background-sync, persistent-storage,
screen-wake-lock, local-fonts, storage-access, idle-detection,
window-management, accelerometer, gyroscope, magnetometer, payment-handler.

Examples:
  chrome-cdp permissions grant notifications
  chrome-cdp permissions grant camera microphone --origin http://localhost:3000
```

## `chrome-cdp permissions deny`

Deny permissions, to every origin or to --origin

```text
chrome-cdp permissions deny [OPTIONS] <NAME>...
```

Arguments:

- `<NAME>`: Permission names, as the Permissions API has them

Options:

- `--origin <ORIGIN>`: Only for this origin (e.g. https://example.com)

```text
Examples:
  chrome-cdp permissions deny geolocation
  chrome-cdp permissions deny notifications --origin https://example.com
```

## `chrome-cdp permissions reset`

Put permissions back to asking (all of them without names)

```text
chrome-cdp permissions reset [OPTIONS] [NAME]...
```

Arguments:

- `<NAME>`: Permission names; every permission without

Options:

- `--origin <ORIGIN>`: Only for this origin

```text
Examples:
  chrome-cdp permissions reset
  chrome-cdp permissions reset notifications
```

