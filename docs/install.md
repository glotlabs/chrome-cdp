# Installing chrome-cdp

chrome-cdp is one static binary for Linux and macOS, on x86_64 and arm64.
Pick any channel below, then run `chrome-cdp doctor` to check the setup.

| Channel | Command | Updates with |
|---|---|---|
| Install script | `curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh \| sh` | `chrome-cdp upgrade` |
| Homebrew | `brew install glotlabs/tap/chrome-cdp` | `brew upgrade chrome-cdp` |
| npm | `npx chrome-cdp …`, or `npm install -g chrome-cdp` | `npm install chrome-cdp@latest` |
| cargo-binstall | `cargo binstall chrome-cdp` | `cargo binstall chrome-cdp` |
| cargo | `cargo install chrome-cdp` (builds from source) | `cargo install chrome-cdp` |
| By hand | a `chrome-cdp-<target>.tar.gz` from the [releases](https://github.com/glotlabs/chrome-cdp/releases) | `chrome-cdp upgrade` |

The targets are `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`
(static, so any distribution), `x86_64-apple-darwin` and
`aarch64-apple-darwin`.

## The install script

`install.sh` picks the archive for this OS and CPU, downloads it and its
`.sha256`, refuses it if the SHA-256 differs, and puts the binary in
`~/.local/bin`. Options go after `sh -s --`:

```sh
curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh | sh -s -- --version 1.2.0
curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh | sh -s -- --dir /usr/local/bin
```

The same settings as environment variables: `CHROME_CDP_VERSION`,
`CHROME_CDP_INSTALL_DIR`, and `CHROME_CDP_RELEASES_URL` for a mirror of the
releases page with the same layout (`latest/download/<file>`,
`download/v<version>/<file>`).

## npm

The `chrome-cdp` package is a small Node launcher. npm installs the native
binary for your platform as an optional dependency (`chrome-cdp-linux-x64`,
`chrome-cdp-linux-arm64`, `chrome-cdp-darwin-x64`,
`chrome-cdp-darwin-arm64`), so an install with `--omit=optional` (or
`--no-optional`) has no binary; the launcher then says so. Add it to a
project's `devDependencies` to pin the version the project's tests use.

## Chrome

chrome-cdp drives the Chrome or Chromium it finds (`--chrome`, then
`$CHROME_CDP_CHROME`, then `chrome-cdp install`'s cache, then a system
Chrome). Without one, `chrome-cdp install` downloads Chrome for Testing;
on Linux, `chrome-cdp install --with-deps` also installs missing system
libraries and, on Ubuntu 23.10+, the AppArmor profile Chrome's sandbox
needs.

## macOS: Gatekeeper and quarantine

Release builds for macOS are signed with a Developer ID and notarized once
the maintainer's signing certificate is set up in the release workflow.
Until then they are unsigned, and a binary downloaded with a browser gets
the `com.apple.quarantine` attribute, so Gatekeeper refuses to run it
("cannot be opened because the developer cannot be verified"). Remove the
attribute:

```sh
xattr -d com.apple.quarantine /path/to/chrome-cdp
```

The install script, Homebrew, npm and cargo do not set the attribute (the
install script also removes it), so this only concerns archives
downloaded by hand. `chrome-cdp doctor` reports a quarantined chrome-cdp or
Chrome, with the command that clears it.

## Checking the setup: `chrome-cdp doctor`

```
$ chrome-cdp doctor
ok    chrome-cdp   chrome-cdp 1.0.0 (git 1a2b3c4d5e6f, CDP protocol r1687809) at /home/ada/.local/bin/chrome-cdp
ok    chrome       Google Chrome for Testing 154.0.8037.57 at … (installed by chrome-cdp install (stable-154.0.8037.57-linux64))
ok    libraries    Chrome finds all its shared libraries
ok    sandbox      the AppArmor profile chrome-cdp lets Chrome's sandbox start
ok    launch       started Chrome/154.0.8037.57 headless and closed it again
ok    session-dir  /run/user/1000/chrome-cdp is private (0700) and ours
skip  quarantine   only macOS quarantines downloads
ok    sessions     no sessions running
7 ok, 0 warnings, 0 failed, 1 skipped
```

Each check prints `ok`, `warn`, `FAIL` or `skip`, and a `fix:` line for
anything not ok. It checks the Chrome that `open` would launch (and that
it is not older than the oldest supported major), on Linux its shared
libraries and Ubuntu's AppArmor rule for its sandbox, a real headless
launch (`--no-launch` skips it), the session directory's owner and mode,
macOS quarantine, and sessions whose daemon is another chrome-cdp version.
A failed check makes it exit with code 4; `--json` lists every check.

## Upgrading: `chrome-cdp upgrade`

`chrome-cdp upgrade` replaces a binary from the install script or an
archive with the newest release: it reads the release's `manifest.json`,
downloads this machine's archive, checks its SHA-256, checks that the new
binary runs, then renames it over the old one. `--check` only says whether
a newer release is out; `--version 1.2.0` installs that release, even an
older one. Sessions still running the old version restart with the new
one on their next command.

A binary that Homebrew, npm or cargo installed is left to that package
manager: `upgrade` prints its command instead.

## Versions

chrome-cdp follows [Semantic Versioning](https://semver.org/). The public
API is the command line (commands, arguments, flags, environment variables,
exit codes, error kinds) and the `--json` output, whose schemas
`chrome-cdp schema <command>` prints: from 1.0 a minor release only adds to
them. The plain text output is for reading and may change in any release.
[CHANGELOG.md](../CHANGELOG.md) lists the changes of each release.
