<!-- Generated from the CLI by `cargo xtask docs`; do not edit. -->
# `chrome-cdp upgrade`

Replace this binary with the newest release (or --version's)

```text
chrome-cdp upgrade [OPTIONS]
```

Options:

- `--check`: Only report whether a newer release exists
- `--version <VERSION>`: Install this release (e.g. 1.2.0) instead of the newest, even an older one
- `--force`: Install even when this version is already running

```text
upgrade downloads the release archive for this OS and CPU from GitHub,
checks its SHA-256 against the release's manifest.json, checks that the new
binary runs, then renames it over this one. Sessions still running the old
build restart with the new one on their next command.

A binary installed by Homebrew, npm or cargo is left alone: upgrade names
the command of that package manager instead. CHROME_CDP_RELEASES_URL
points at a mirror of https://github.com/glotlabs/chrome-cdp/releases.

Examples:
  chrome-cdp upgrade
  chrome-cdp upgrade --check          # is there a newer release?
  chrome-cdp upgrade --version 1.2.0  # that release, even an older one
```

