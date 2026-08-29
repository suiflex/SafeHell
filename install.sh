#!/bin/sh
# SafeHell installer.
#
#   curl -fsSL https://raw.githubusercontent.com/suiflex/SafeHell/develop/install.sh | sh
#
# Environment overrides:
#   SAFEHELL_VERSION      release tag to install (default: latest)
#   SAFEHELL_INSTALL_DIR  destination directory (default: $HOME/.local/bin)

set -eu

REPO="suiflex/SafeHell"

err() {
	printf 'safehell: %s\n' "$*" >&2
	exit 1
}

need() {
	command -v "$1" >/dev/null 2>&1 || err "'$1' is required but was not found"
}

# Maps uname output to a release asset name. Every supported pair must match an
# asset produced by .github/workflows/release.yml.
detect_asset() {
	os=$(uname -s)
	arch=$(uname -m)

	case "$os" in
	Linux) os=linux ;;
	Darwin) os=macos ;;
	MINGW* | MSYS* | CYGWIN*)
		err "Windows is not supported by this script; download safehell-windows-x86_64.exe from https://github.com/$REPO/releases"
		;;
	*) err "unsupported operating system: $os" ;;
	esac

	# Under Rosetta, uname reports x86_64 on Apple Silicon, which would install
	# the translated binary and keep reinstalling it on every later update.
	if [ "$os" = macos ] && [ "$arch" = x86_64 ] &&
		[ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = 1 ]; then
		arch=arm64
	fi

	case "$arch" in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) err "unsupported architecture: $arch" ;;
	esac

	printf 'safehell-%s-%s' "$os" "$arch"
}

# Resolves the latest tag by following the /releases/latest redirect, which
# avoids depending on jq to parse the API response.
latest_version() {
	url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
		err "could not reach GitHub to resolve the latest release"
	tag=${url##*/}
	if [ -z "$tag" ] || [ "$tag" = "releases" ]; then
		err "no published release found for $REPO"
	fi
	printf '%s' "$tag"
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 "$1" | cut -d' ' -f1
	else
		err "neither 'sha256sum' nor 'shasum' is available to verify the download"
	fi
}

main() {
	need curl
	need uname

	asset=$(detect_asset)
	version=${SAFEHELL_VERSION:-$(latest_version)}
	install_dir=${SAFEHELL_INSTALL_DIR:-$HOME/.local/bin}
	base="https://github.com/$REPO/releases/download/$version"

	tmp=$(mktemp -d)
	trap 'rm -rf "$tmp"' EXIT INT TERM

	printf 'Downloading %s %s\n' "$asset" "$version"
	curl -fsSL -o "$tmp/$asset" "$base/$asset" ||
		err "no asset '$asset' in release $version; see https://github.com/$REPO/releases"
	curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" ||
		err "release $version has no SHA256SUMS; refusing to install an unverified binary"

	expected=$(grep " \{1,2\}\*\{0,1\}$asset\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)
	[ -n "$expected" ] || err "SHA256SUMS has no entry for $asset"
	actual=$(sha256_of "$tmp/$asset")
	[ "$actual" = "$expected" ] || err "checksum mismatch for $asset (expected $expected, got $actual)"

	mkdir -p "$install_dir"
	chmod +x "$tmp/$asset"
	mv -f "$tmp/$asset" "$install_dir/safehell"

	printf 'Installed safehell %s to %s/safehell\n' "$version" "$install_dir"

	case ":$PATH:" in
	*":$install_dir:"*) ;;
	*) printf '\n%s is not on your PATH. Add it, then run: safehell setup\n' "$install_dir" ;;
	esac
}

main "$@"
