<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp install`

Download Chrome for Testing into chrome-cdp's cache

```text
chrome-cdp install [OPTIONS]
```

Options:

- `--channel <CHANNEL>`: The release channel to install (one of `stable`, `beta`, `dev`, `canary`; default `stable`)
- `--version <VERSION>`: A version, or the start of one: 154, 154.0.8037.57
- `--headless-shell`: Install chrome-headless-shell (smaller, headless only) instead of full Chrome
- `--force`: Download again even if it is installed
- `--with-deps`: Linux: also install the system libraries Chrome needs (sudo apt-get or dnf) and, on Ubuntu 23.10+, the AppArmor profile its sandbox needs. Asks first on a terminal
- `--list`: List the installed browsers and each channel's current version

```text
The cache is $CHROME_CDP_CACHE, else ~/.cache/chrome-cdp (Linux) or
~/Library/Caches/chrome-cdp (macOS). --timeout bounds connecting and each
wait for data, not the whole download. CHROME_CDP_CFT_URL points at a
mirror of the Chrome for Testing index.

Examples:
  chrome-cdp install                       # the current stable Chrome
  chrome-cdp install --headless-shell      # smaller, headless only
  chrome-cdp install --channel beta
  chrome-cdp install --version 154         # the newest 154.x
  chrome-cdp install --with-deps           # Linux: also the system libraries
  chrome-cdp install --list
```

