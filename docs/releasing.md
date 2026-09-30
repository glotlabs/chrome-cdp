# Releasing chrome-cdp

For maintainers. A release is a `vX.Y.Z` tag; `.github/workflows/release.yml`
does the rest. [docs/install.md](install.md) is the user side.

## Cutting a release

1. In [CHANGELOG.md](../CHANGELOG.md), move the Unreleased entries under
   `## [X.Y.Z] - YYYY-MM-DD`.
2. Set `version = "X.Y.Z"` in `Cargo.toml`: in `[workspace.package]` and in
   each internal dependency (`package = "chrome-cdp-…"`, which crates.io
   needs), then run `cargo check` to update `Cargo.lock`.
3. `cargo xtask release check --tag vX.Y.Z` checks both, and that the
   release only adds to the previous one's API (below).
4. Commit, tag `vX.Y.Z` and push the tag. A tag with a `-` (`v1.0.0-rc.1`)
   makes a pre-release: npm's `next` tag, and no Homebrew update.

The workflow then:

- checks the versions and the compatibility (`release check --tag`);
- builds the four targets, each on its own architecture, with
  `cargo xtask dist` (the +20% size gate) and runs each binary;
- on macOS, signs and notarizes the binary when the Apple secrets are set;
- packs `chrome-cdp-<target>.tar.gz` and its `.sha256`
  (`cargo xtask release archive`);
- publishes the GitHub release: the archives, `SHA256SUMS`,
  `manifest.json` (read by `chrome-cdp upgrade`), `install.sh`, and notes
  made of the CHANGELOG section plus the commits since the previous tag,
  grouped by their conventional-commit type (`release notes`);
- updates the Homebrew tap, publishes to npm and to crates.io, each only
  when its secret is set (a missing one is a warning, not a failure).

To redo a release (a failed step, a new secret), run the workflow by hand
with the tag: it replaces the GitHub release's assets and notes, and the
other jobs run again.

The CI workflow runs every step but publishing on each push (the
`release packaging` job), so a release does not find a broken step.

## Compatibility check

[output.md](output.md#versions-and-stability) promises that from 1.0 a
minor release only adds to the public API. `release check` holds releases
to it: it compares, in the working tree and at the previous `v*` tag
(`git describe` from the tag's parent, or from HEAD without `--tag`):

- each `--json` schema, `crates/chrome-cdp/tests/golden/schema/*.json`.
  Breaking: a removed (or renamed) property, a changed type (any change
  to the JSON types a value may have, `null` included), a new required
  property or one no longer required, a removed enum value, error kind or
  `oneOf`/`anyOf` variant (tagged variants are paired by their `type`),
  a removed schema. `$ref`s are followed, so a renamed definition is not
  a change;
- the command line, `crates/chrome-cdp/tests/golden/cli.txt` (every
  command of `help commands` and their subcommands, with their arguments,
  flags, values and environment variables; its test regenerates it).
  Breaking: a removed command, argument, flag, short flag, flag value or
  environment variable, a flag that stops or starts taking a value or
  stops being repeatable, a new required argument or flag. A flag moved
  to the global flags is not removed.

New properties, commands, flags and values, and changed descriptions and
help text, are fine. A breaking change fails the check with one line per
change, unless the major version went up (before 1.0, the minor one, as
Cargo reads `0.y.z`); then it is only a warning, to copy into the
CHANGELOG's breaking changes. Without a previous tag (the first release)
the check says so and skips; a tag from before `cli.txt` existed has only
its schemas compared.

Without `--tag` it compares HEAD with the newest tag, so between
releases a breaking change fails it until `Cargo.toml` has the next
major version. `release check --since <rev>` compares with any commit
instead of a tag, so the schemas can be checked before the first
release too (for example since the commit before a plan's work); there
any break fails.

## Secrets

| Secret | For | Without it |
|---|---|---|
| `APPLE_CERTIFICATE` | a Developer ID Application certificate and key, as a base64 `.p12` | macOS binaries ship unsigned (the `xattr` workaround in install.md) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12`'s password | |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Glot Labs (TEAMID)` | |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | notarization (an app-specific password of the Apple ID) | signed, not notarized: Gatekeeper still asks on first run |
| `HOMEBREW_TAP_TOKEN` | a token that can push to `glotlabs/homebrew-tap` | the tap is not updated |
| `NPM_TOKEN` | an npm automation token that can publish `chrome-cdp` and the four `chrome-cdp-<os>-<cpu>` packages | nothing goes to npm |
| `CARGO_REGISTRY_TOKEN` | a crates.io token for `chrome-cdp` and the `chrome-cdp-*` crates | nothing goes to crates.io |

`.github/scripts/macos-sign.sh` does the signing: it imports the
certificate into a temporary keychain, signs with the hardened runtime and
a timestamp, and submits a zip of the binary to `notarytool`. A bare binary
cannot be stapled, so Gatekeeper checks the notarization online on first
run.

## Before the first release

- **License.** None is chosen yet. Add `license = "…"` to
  `[workspace.package]` (each crate inherits it through
  `license.workspace = true`, which has to be added to the eight published
  crates) and the license file(s) at the top of the repo; `release archive`
  packs `LICENSE`, `LICENSE-MIT` and `LICENSE-APACHE` when they exist.
  npm and crates.io need it.
- **crates.io.** Publishing is off (`publish = false` in
  `[workspace.package]`) until the license is in, because cargo-deny fails
  on publishable crates without one. Set `publish = true` with the license.
  The crates are `chrome-cdp` and `chrome-cdp-{cdp,protocol,browser,mock,
  engine,daemon,e2e}`; `cargo publish --workspace` publishes them in
  dependency order.
- **Homebrew.** Create the `glotlabs/homebrew-tap` repository (with a
  `Formula/` directory); the workflow writes `Formula/chrome-cdp.rb`
  (`cargo xtask release formula`) there.
- **npm.** The package names `chrome-cdp` and `chrome-cdp-<os>-<cpu>` must
  be free or owned by the token's account. The workflow publishes with
  provenance, which needs the repository to be public.
