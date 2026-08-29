#!/bin/sh
# Acceptance checks for install.sh. No network: the release is served from a
# temporary directory over file://.
#
#   sh tests/install_sh_test.sh

set -eu
unset CDPATH

script=$(cd -- "$(dirname -- "$0")/.." && pwd)/install.sh
[ -f "$script" ] || { echo "install.sh not found at $script" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

pass=0
fail=0
check() {
	if [ "$2" = "$3" ]; then
		pass=$((pass + 1))
		echo "ok   $1"
	else
		fail=$((fail + 1))
		echo "FAIL $1"
		echo "       expected: $3"
		echo "       actual:   $2"
	fi
}

# --- unit: platform detection and checksum handling -------------------------
# install.sh ends with `main "$@"`; dropping that line makes it sourceable.
sed '$d' "$script" >"$work/lib.sh"
grep -q '^main "\$@"$' "$script" || { echo "install.sh no longer ends with main \"\$@\"" >&2; exit 1; }

# shellcheck source=/dev/null
. "$work/lib.sh"

# Stub uname and sysctl so detect_asset can be driven across platforms.
uname() { if [ "$1" = "-s" ]; then echo "$T_OS"; else echo "$T_ARCH"; fi; }
sysctl() { echo "${T_ARM64:-0}"; }

T_ARM64=0
T_OS=Linux T_ARCH=x86_64 && check "linux/x86_64" "$(detect_asset)" "safehell-linux-x86_64"
T_OS=Linux T_ARCH=amd64 && check "linux/amd64" "$(detect_asset)" "safehell-linux-x86_64"
T_OS=Linux T_ARCH=aarch64 && check "linux/aarch64" "$(detect_asset)" "safehell-linux-aarch64"
T_OS=Darwin T_ARCH=arm64 && check "darwin/arm64" "$(detect_asset)" "safehell-macos-aarch64"
T_OS=Darwin T_ARCH=x86_64 && check "darwin/x86_64" "$(detect_asset)" "safehell-macos-x86_64"
T_OS=Darwin T_ARCH=x86_64 T_ARM64=1 &&
	check "darwin/rosetta" "$(detect_asset)" "safehell-macos-aarch64"
T_ARM64=0
T_OS=Linux T_ARCH=riscv64 &&
	check "rejects unknown arch" "$(detect_asset 2>&1 || true)" "safehell: unsupported architecture: riscv64"
T_OS=Plan9 T_ARCH=x86_64 &&
	check "rejects unknown os" "$(detect_asset 2>&1 || true)" "safehell: unsupported operating system: Plan9"
T_OS=MINGW64_NT-10.0 T_ARCH=x86_64 &&
	check "points Windows at the exe" "$(detect_asset 2>&1 | cut -d';' -f1 || true)" \
		"safehell: Windows is not supported by this script"

printf 'abc' >"$work/vec"
check "sha256 known vector" "$(sha256_of "$work/vec")" \
	"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"

# The SHA256SUMS lookup must anchor on the full asset name and tolerate the
# `*` binary-mode marker that sha256sum emits on some platforms.
cat >"$work/SHA256SUMS" <<'SUMS'
aaa11  safehell-linux-x86_64
bbb22  safehell-linux-aarch64
ccc33 *safehell-macos-aarch64
SUMS
look() { grep " \{1,2\}\*\{0,1\}$1\$" "$work/SHA256SUMS" | cut -d' ' -f1; }
check "sums exact match" "$(look safehell-linux-x86_64)" "aaa11"
check "sums aarch64 entry" "$(look safehell-linux-aarch64)" "bbb22"
check "sums binary-mode entry" "$(look safehell-macos-aarch64)" "ccc33"
check "sums missing asset" "$(look safehell-macos-x86_64)" ""
check "sums no prefix bleed" "$(look safehell-linux)" ""

# --- end to end against a file:// release -----------------------------------
asset=$(T_OS=$(command uname -s) T_ARCH=$(command uname -m) detect_asset)
rel="$work/rel/download/v9.9.9"
bin="$work/bin"
mkdir -p "$rel"
printf '#!/bin/sh\necho installed-ok\n' >"$rel/$asset"
(cd "$rel" && (sha256sum safehell-* 2>/dev/null || shasum -a 256 safehell-*) >SHA256SUMS)

# shellcheck disable=SC2016  # matched literally against install.sh, not expanded
remote='https://github.com/$REPO/releases/download/$version'
grep -qF "$remote" "$script" || { echo "install.sh download base URL changed" >&2; exit 1; }
sed "s|$remote|file://$work/rel/download/\$version|" "$script" >"$work/local.sh"

run() { SAFEHELL_VERSION=v9.9.9 SAFEHELL_INSTALL_DIR="$bin" sh "$work/local.sh" 2>&1 | grep -v '^curl:' || true; }

out=$(run)
check "installs the binary" "$(sh "$bin/safehell")" "installed-ok"
check "warns when dir is off PATH" "$(echo "$out" | grep -c 'not on your PATH')" "1"

# A modified binary must be rejected even though the download itself succeeds.
printf 'tampered' >"$rel/$asset"
check "rejects checksum mismatch" "$(run | grep -c '^safehell: checksum mismatch')" "1"

rm "$rel/SHA256SUMS"
check "refuses without SHA256SUMS" "$(run | grep -c 'refusing to install an unverified binary')" "1"

echo "---"
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
