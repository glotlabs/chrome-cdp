#!/bin/sh
# Installs chrome-cdp from its GitHub releases (plan 10, milestone 10.5):
#
#   curl -fsSL https://github.com/glotlabs/chrome-cdp/releases/latest/download/install.sh | sh
#
# It picks the archive for this OS and CPU (Linux or macOS, x86_64 or
# arm64), checks it against its published SHA-256, and puts the binary in
# ~/.local/bin. Pass options after `sh -s --`:
#
#   --version X.Y.Z   a release other than the newest   (CHROME_CDP_VERSION)
#   --dir DIR         where to put the binary            (CHROME_CDP_INSTALL_DIR)
#
# CHROME_CDP_RELEASES_URL replaces https://github.com/glotlabs/chrome-cdp/releases
# (a mirror with the same layout).
#
# Everything runs from `main` at the end, so a download cut short runs
# nothing.

set -eu

REPO_RELEASES="https://github.com/glotlabs/chrome-cdp/releases"

say() {
	printf '%s\n' "$*"
}

fail() {
	printf 'error: %s\n' "$1" >&2
	if [ $# -gt 1 ]; then
		printf '  hint: %s\n' "$2" >&2
	fi
	exit 1
}

usage() {
	say "usage: install.sh [--version X.Y.Z] [--dir DIR]"
}

# The release target of this machine, e.g. x86_64-unknown-linux-musl.
detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$arch" in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) fail "chrome-cdp has no build for the CPU $arch" "it runs on x86_64 and arm64" ;;
	esac
	case "$os" in
	Linux) printf '%s-unknown-linux-musl\n' "$arch" ;;
	Darwin)
		# An x86_64 shell under Rosetta on Apple silicon: take the native build.
		if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
			arch=aarch64
		fi
		printf '%s-apple-darwin\n' "$arch"
		;;
	*) fail "chrome-cdp has no build for $os" "it runs on Linux and macOS" ;;
	esac
}

download() {
	url=$1
	out=$2
	# HTTPS, and no redirect off it; a mirror may also be a local dir
	# (file://) or plain HTTP on this machine.
	case "$url" in
	https://*) proto='=https' ;;
	file://*) proto='=file' ;;
	http://127.0.0.1[:/]* | http://localhost[:/]* | http://\[::1\][:/]*) proto='=http' ;;
	*) fail "$url is not HTTPS" "downloads use HTTPS, or HTTP only to this machine" ;;
	esac
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL --proto "$proto" --proto-redir "$proto" --tlsv1.2 --retry 3 -o "$out" "$url" || fail "cannot download $url" "check the version, the network, or the releases at $REPO_RELEASES"
	elif command -v wget >/dev/null 2>&1; then
		wget -q -O "$out" "$url" || fail "cannot download $url" "check the version, the network, or the releases at $REPO_RELEASES"
	else
		fail "neither curl nor wget is installed" "install one of them and run this again"
	fi
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d ' ' -f 1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d ' ' -f 1
	elif command -v openssl >/dev/null 2>&1; then
		openssl dgst -sha256 "$1" | sed 's/^.*= //'
	else
		fail "no sha256sum, shasum or openssl to check the download with"
	fi
}

main() {
	version=${CHROME_CDP_VERSION:-}
	dir=${CHROME_CDP_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}
	while [ $# -gt 0 ]; do
		case "$1" in
		--version)
			[ $# -ge 2 ] || fail "--version needs a value"
			version=$2
			shift 2
			;;
		--dir)
			[ $# -ge 2 ] || fail "--dir needs a value"
			dir=$2
			shift 2
			;;
		-h | --help)
			usage
			return 0
			;;
		*)
			usage >&2
			fail "unknown option $1"
			;;
		esac
	done
	version=${version#v}

	target=$(detect_target)
	archive="chrome-cdp-$target.tar.gz"
	base=${CHROME_CDP_RELEASES_URL:-$REPO_RELEASES}
	base=${base%/}
	if [ -n "$version" ]; then
		url="$base/download/v$version/$archive"
	else
		url="$base/latest/download/$archive"
	fi

	tmp=$(mktemp -d 2>/dev/null || mktemp -d -t chrome-cdp)
	trap 'rm -rf "$tmp"' EXIT INT TERM

	say "downloading $url"
	download "$url" "$tmp/$archive"
	download "$url.sha256" "$tmp/$archive.sha256"

	expected=$(cut -d ' ' -f 1 <"$tmp/$archive.sha256")
	actual=$(sha256_of "$tmp/$archive")
	if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
		fail "the checksum of $archive does not match: expected ${expected:-nothing}, got $actual" \
			"download it again; if it keeps failing, report it at https://github.com/glotlabs/chrome-cdp/issues"
	fi

	tar -xzf "$tmp/$archive" -C "$tmp"
	binary="$tmp/chrome-cdp-$target/chrome-cdp"
	[ -f "$binary" ] || fail "$archive holds no chrome-cdp-$target/chrome-cdp"

	mkdir -p "$dir" || fail "cannot create $dir" "pass --dir with a directory you can write to"
	# Copy next to the target, then rename: a running chrome-cdp keeps its
	# old file, and nobody sees a half-written one.
	cp "$binary" "$dir/.chrome-cdp.$$" || fail "cannot write to $dir" "pass --dir with a directory you can write to"
	chmod 755 "$dir/.chrome-cdp.$$"
	if [ "$(uname -s)" = Darwin ]; then
		xattr -d com.apple.quarantine "$dir/.chrome-cdp.$$" 2>/dev/null || true
	fi
	mv -f "$dir/.chrome-cdp.$$" "$dir/chrome-cdp"

	installed=$("$dir/chrome-cdp" --version 2>/dev/null) ||
		fail "$dir/chrome-cdp was installed but does not run" "run chrome-cdp doctor to see why"
	say "installed $installed to $dir/chrome-cdp"
	case ":${PATH:-}:" in
	*":$dir:"*) ;;
	*) say "note: $dir is not on your PATH; add it, e.g.: export PATH=\"$dir:\$PATH\"" ;;
	esac
	say "next: chrome-cdp doctor   (checks for a Chrome; chrome-cdp install gets one)"
}

main "$@"
