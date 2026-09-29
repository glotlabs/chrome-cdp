#!/bin/sh
# Signs a macOS binary with a Developer ID certificate and, when the Apple
# ID credentials are there too, notarizes it (plan 10.5), so Gatekeeper
# lets a downloaded chrome-cdp run. The release workflow runs it only when
# the APPLE_CERTIFICATE secret is set; without it the binaries ship
# unsigned (docs/install.md has the xattr workaround).
#
#   macos-sign.sh BINARY
#
# Signing:      APPLE_CERTIFICATE (base64 .p12), APPLE_CERTIFICATE_PASSWORD,
#               APPLE_SIGNING_IDENTITY ("Developer ID Application: … (TEAMID)")
# Notarizing:   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD (an app-specific
#               password)
#
# A bare Mach-O binary cannot be stapled: Gatekeeper looks the ticket up
# online on first run.

set -eu

binary=$1
: "${APPLE_CERTIFICATE:?}" "${APPLE_CERTIFICATE_PASSWORD:?}" "${APPLE_SIGNING_IDENTITY:?}"

work=$(mktemp -d)
keychain="$work/signing.keychain-db"
keychain_password=$(openssl rand -hex 24)
cleanup() {
	security delete-keychain "$keychain" 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode >"$work/cert.p12"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 3600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/cert.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" \
	-T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
	-k "$keychain_password" "$keychain" >/dev/null
# codesign searches the user's keychain list: put ours first.
# shellcheck disable=SC2046
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')

codesign --force --timestamp --options runtime --keychain "$keychain" \
	--sign "$APPLE_SIGNING_IDENTITY" "$binary"
codesign --verify --strict --verbose=2 "$binary"
echo "signed $binary"

if [ -z "${APPLE_ID:-}" ] || [ -z "${APPLE_TEAM_ID:-}" ] || [ -z "${APPLE_APP_PASSWORD:-}" ]; then
	echo "::warning::APPLE_ID, APPLE_TEAM_ID or APPLE_APP_PASSWORD is not set: $binary is signed but not notarized"
	exit 0
fi
ditto -c -k --keepParent "$binary" "$work/notarize.zip"
auth() {
	"$@" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD"
}
result=$(auth xcrun notarytool submit "$work/notarize.zip" --wait --timeout 30m --output-format json) || true
echo "$result"
case "$result" in
*'"status":"Accepted"'*) echo "notarized $binary" ;;
*)
	id=$(printf '%s' "$result" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
	if [ -n "$id" ]; then
		auth xcrun notarytool log "$id" || true
	fi
	echo "::error::notarizing $binary failed"
	exit 1
	;;
esac
