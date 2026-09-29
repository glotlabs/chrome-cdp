<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp geo`

Set where navigator.geolocation says the device is

```text
chrome-cdp geo [OPTIONS] <LAT,LON>
```

Arguments:

- `<LAT,LON>`: LATITUDE,LONGITUDE in degrees (e.g. 59.9139,10.7522), or `reset`

Options:

- `--accuracy <METRES>`: How precise the position is, in metres (default `10`)

```text
Also grants the geolocation permission to every origin, so pages get the
position without asking. `geo reset` stops emulating the position.

Examples:
  chrome-cdp geo 59.9139,10.7522
  chrome-cdp geo -33.8688,151.2093 --accuracy 50
  chrome-cdp geo reset
```

