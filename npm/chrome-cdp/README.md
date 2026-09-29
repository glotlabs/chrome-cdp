# chrome-cdp

Drive Chrome over the DevTools Protocol from a shell; built for AI agents.

```sh
npx chrome-cdp open https://example.com
npx chrome-cdp snapshot -i
```

This package is a small launcher. npm installs the native binary for your
platform (Linux and macOS, x64 and arm64) as an optional dependency,
`chrome-cdp-<os>-<cpu>`. Installing with `--omit=optional` leaves it out.

`chrome-cdp upgrade` does not replace an npm install: update it with
`npm install chrome-cdp@latest`.

Documentation: https://github.com/glotlabs/chrome-cdp
