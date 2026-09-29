# Changelog

All notable changes to chrome-cdp are recorded here, in the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

chrome-cdp follows [Semantic Versioning](https://semver.org/). Its public
API is the command line (commands, arguments, flags, environment
variables, exit codes and error kinds) and the `--json` output, whose
schemas `chrome-cdp schema <command>` prints: from 1.0, a minor release
may only add to them, and anything else needs a major release. The plain
text output is for reading and may change in any release.

To release: move the Unreleased entries under a `## [X.Y.Z] - date`
heading, set that version in `Cargo.toml` (`[workspace.package]` and the
internal dependencies), then push the tag `vX.Y.Z`. The release workflow
checks both with `cargo xtask release check --tag vX.Y.Z`, and puts the
section at the top of the GitHub release notes.

## [Unreleased]

### Added

- The first release: every command of plans 01-11 (see `chrome-cdp help`).
- Release archives for Linux and macOS (x86_64 and arm64) with SHA-256
  sums, installable with `install.sh`, Homebrew, `cargo install`,
  `cargo binstall` or npm (`npx chrome-cdp`).
- `chrome-cdp upgrade` updates a standalone install from the GitHub
  releases, checking the archive's checksum.
- `chrome-cdp doctor` checks Chrome, its libraries and sandbox, the
  session directory, macOS quarantine and version skew, and prints fixes.
