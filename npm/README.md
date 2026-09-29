# npm packages

`chrome-cdp/` is the template of the `chrome-cdp` npm package: a Node
launcher (`bin/chrome-cdp.js`) that runs the binary from the platform
package npm installed as an optional dependency. Its `0.0.0-dev` versions
are placeholders.

At release, `cargo xtask release npm` (after `release archive` for each
target) writes to `target/npm/`:

- `chrome-cdp/`, this package with the release version;
- `chrome-cdp-<os>-<cpu>/` for linux/darwin and x64/arm64, holding
  `bin/chrome-cdp` from the target's archive, with `os` and `cpu` set so
  npm installs only the matching one.

The release workflow publishes the platform packages first, then
`chrome-cdp` (only when the `NPM_TOKEN` secret is set).
