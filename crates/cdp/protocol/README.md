# Vendored Chrome DevTools Protocol

`browser_protocol.json` and `js_protocol.json` come from the
[devtools-protocol](https://github.com/ChromeDevTools/devtools-protocol)
repository, pinned to the Chromium revision in `PROTOCOL_VERSION`.
`domains.txt` lists the domains `cargo xtask gen-cdp` turns into Rust modules
under `crates/cdp/src/domains/`.

## Updating

1. Pick a revision: the latest `v0.0.<rev>` tag of devtools-protocol at or
   below the Chrome build's `chromium_main_branch_position`
   (`https://chromiumdash.appspot.com/fetch_version?version=<chrome version>`).
2. Fetch the two files:

   ```sh
   rev=1687809
   for f in browser_protocol js_protocol; do
     curl -sSfL -o crates/cdp/protocol/$f.json \
       https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/v0.0.$rev/json/$f.json
   done
   ```

3. Update `PROTOCOL_VERSION`, then run `cargo xtask gen-cdp` and commit the
   JSON, `PROTOCOL_VERSION` and the regenerated code together.

## Adding a domain

Add one line to `domains.txt` and run `cargo xtask gen-cdp`. A line
`Domain: a, b` generates only the commands and events `a` and `b` (plus the
types they use). Types from domains that are not listed are still generated
when a listed domain refers to them.
