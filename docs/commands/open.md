<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp open`

Start the session's browser (or check the running one) and optionally open a URL

```text
chrome-cdp open [OPTIONS] [URL]
```

Arguments:

- `<URL>`: A URL to open in the current page; without a scheme, https:// (or http:// for localhost) is added

Options:

- `--chrome <PATH>`: The Chrome or Chromium binary to launch. Default: $CHROME_CDP_CHROME, then browsers from `chrome-cdp install`, then a system Chrome
- `--auto-install`: If no Chrome is found, install Chrome for Testing first (env `CHROME_CDP_AUTO_INSTALL`)
- `--headed`: Open a browser window instead of running headless
- `--profile <NAME>`: Keep the profile (logins, cookies, storage) between runs under this name. Default: a temporary profile, deleted on close
- `--user-data-dir <PATH>`: Use this directory as Chrome's profile (never your everyday one)
- `--chrome-arg <FLAG>`: An extra flag for Chrome, e.g. --chrome-arg=--window-size=1280,800; repeat it for more. docs/chrome-flags.md lists chrome-cdp's own
- `--fake-media`: Give pages a fake camera and microphone, and allow them without a prompt (--use-fake-device-for-media-stream, --use-fake-ui-for-media-stream)
- `--extension <DIR>`: Load the unpacked extension in this directory (the one holding its manifest.json); repeat it for more. Headless or headed, but not with chrome-headless-shell
- `--restart`: Close a running browser first and start one with these options
- `--device <DEVICE>`: Emulate this device (as `emulate <device>`) before loading URL. The session keeps it, like `emulate`
- `--init-script <FILE>`: Run this file's script in every document before its own scripts, from URL on (as `init-script add FILE`). Repeatable; the session keeps them, and `init-script list` shows them
- `--init-script-code <JS>`: Like --init-script, with the script itself. Repeatable; these run after the --init-script files
- `--state <FILE>`: Load cookies and storage from a state file (as `state load`) before loading URL
- `--follow-popups`: Make popups (window.open, target=_blank) the active tab as they open. The session keeps this setting
- `--ignore-https-errors`: Load pages despite certificate errors (self-signed test servers). The session keeps this setting; it applies to a running browser too
- `--bypass-service-worker`: Send requests past service workers to the network, even without routes (routes always do, since a service worker would answer before them). The session keeps this setting; it applies to a running browser too
- `--dialogs <POLICY>`: How to answer JavaScript dialogs. The session keeps this setting (one of `auto`, `accept`, `dismiss`, `manual`)
- `--downloads-dir <DIR>`: Save downloads in this directory (created; relative to the current one) instead of the session's. The session keeps this setting; it applies to a running browser too
- `--http-credentials <USER:PASSWORD>`: Answer HTTP authentication (basic, digest) with USER:PASSWORD; `-` reads it from stdin. The session keeps it; it applies to a running browser too. Every origin that asks gets it, unless --http-credentials-origin names one (env `CHROME_CDP_HTTP_CREDENTIALS`)
- `--http-credentials-origin <ORIGIN>`: Send --http-credentials only to this origin (e.g. https://app.test); other origins' challenges are cancelled, so a third-party frame or a redirect elsewhere never gets them (env `CHROME_CDP_HTTP_CREDENTIALS_ORIGIN`)
- `--console-buffer <N>`: How many console entries each tab keeps (default 1,000). The session keeps this setting (env `CHROME_CDP_CONSOLE_BUFFER`)
- `--network-buffer <N>`: How many requests each tab's network log keeps (default 2,000). The session keeps this setting (env `CHROME_CDP_NETWORK_BUFFER`)
- `--node-stacks <WHERE>`: Where Chrome records the stack that creates each DOM node, to tell which line of the page's code made an element. Recording costs page time, so by default only pages on this machine get it (`node_stacks` in the config sets the default). The session keeps this setting; it applies to a running browser too, for the nodes made from then on (one of `on`, `off`, `localhost`; env `CHROME_CDP_NODE_STACKS`)
- `--idle-timeout <DURATION>`: Close the browser and stop the daemon after this long without commands, e.g. 10m or 2h; 0 means never. Default 30m (env `CHROME_CDP_IDLE_TIMEOUT`)

```text
Each session (--session, default `default`) has its own background daemon
and browser; later commands reuse them. Running open again is a no-op,
unless it asks for other launch options: add --restart for those.
The daemon closes the browser after --idle-timeout without commands.
An --extension is loaded before the URL; open prints its id, so its pages
(popup, options) open with goto chrome-extension://<id>/popup.html.

Examples:
  chrome-cdp open https://example.com
  chrome-cdp open --headed                          # with a window
  chrome-cdp open --profile work https://app.test   # keeps logins
  chrome-cdp open --session shop http://localhost:3000
  chrome-cdp open --device 'iPhone 15' http://localhost:3000
  chrome-cdp open --init-script ./stub-analytics.js http://localhost:3000
  chrome-cdp open --restart --chrome-arg=--window-size=1920,1080
  chrome-cdp open --follow-popups                   # popups become the active tab
  chrome-cdp open --dialogs dismiss                 # answer every dialog with Cancel
  chrome-cdp open --downloads-dir ./downloads       # downloads land there
  chrome-cdp open --http-credentials ada:s3cret http://localhost:8080/admin
  chrome-cdp open --node-stacks on https://staging.app.test   # record who made each node
  chrome-cdp open --extension ./my-extension         # prints its chrome-extension:// origin
```

