#!/bin/sh
# Acceptance checks for the Homebrew and Scoop renderers.
#
# .github/workflows/release-build.yml renders these and pushes the result
# straight to suiflex/homebrew-tap and suiflex/scoop-bucket. Nothing else reads
# them, so without this a broken render reaches a user's `brew install` before
# anyone notices.
#
# No network: the release assets are faked with small files, since only their
# presence and digest matter to the renderers.

set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

pass=0
fail=0
ok() {
	pass=$((pass + 1))
	printf 'ok   %s\n' "$1"
}
no() {
	fail=$((fail + 1))
	printf 'FAIL %s\n' "$1" >&2
}

sha_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	else
		shasum -a 256 "$1" | cut -d' ' -f1
	fi
}

# ---------------------------------------------------------------- scoop
# The manifest is built rather than templated, so the checks are about which
# architectures appear and whether a missing required asset is refused.

cd "$tmp"
printf 'x86' >safehell-windows-x86_64.zip
printf 'arm' >safehell-windows-aarch64.zip

if TAG=v0.2.0 REPO=suiflex/SafeHell python3 "$root/packaging/render_scoop.py" >both.json 2>/dev/null; then
	if python3 -c "
import json,sys
d=json.load(open('both.json'))
assert set(d['architecture']) == {'64bit','arm64'}, d['architecture']
assert all(len(v['hash'])==64 for v in d['architecture'].values())
assert d['version']=='0.2.0', d['version']
" 2>/dev/null; then
		ok "scoop both architectures"
	else
		no "scoop both architectures"
	fi
else
	no "scoop both architectures (renderer exited non-zero)"
fi

rm safehell-windows-aarch64.zip
if TAG=v0.2.0 REPO=suiflex/SafeHell python3 "$root/packaging/render_scoop.py" >one.json 2>/dev/null; then
	if python3 -c "
import json,sys
d=json.load(open('one.json'))
assert set(d['architecture']) == {'64bit'}, d['architecture']
" 2>/dev/null; then
		ok "scoop degrades to 64bit when arm64 is missing"
	else
		no "scoop degrades to 64bit when arm64 is missing"
	fi
else
	no "scoop degrades to 64bit when arm64 is missing (renderer exited non-zero)"
fi

# x86_64 is not a best-effort leg. Its absence means the build broke, and a
# manifest without it would silently strand every scoop user on the old version.
rm safehell-windows-x86_64.zip
if TAG=v0.2.0 REPO=suiflex/SafeHell python3 "$root/packaging/render_scoop.py" >/dev/null 2>&1; then
	no "scoop refuses to render without the 64bit archive"
else
	ok "scoop refuses to render without the 64bit archive"
fi

# ------------------------------------------------------------- homebrew
# The formula IS sed-templated, so these checks mirror the validation the
# release workflow runs before pushing to the tap.

digest=$(printf 'formula-fixture' >f.bin && sha_of f.bin)

render_formula() {
	# $1: "with" or "without" the best-effort arm64 Linux stanza
	if [ "$1" = with ]; then
		sed -e "s|@VERSION@|0.2.0|g" -e "s|@BASE@|https://example.invalid/dl|g" \
			-e "s|@MACOS_ARM64_SHA@|$digest|g" -e "s|@MACOS_X86_64_SHA@|$digest|g" \
			-e "s|@LINUX_X86_64_SHA@|$digest|g" -e "s|@LINUX_ARM64_SHA@|$digest|g" \
			-e "s| # @LINUX_ARM64@||g" \
			"$root/packaging/safehell.rb.tmpl"
	else
		sed -e "s|@VERSION@|0.2.0|g" -e "s|@BASE@|https://example.invalid/dl|g" \
			-e "s|@MACOS_ARM64_SHA@|$digest|g" -e "s|@MACOS_X86_64_SHA@|$digest|g" \
			-e "s|@LINUX_X86_64_SHA@|$digest|g" \
			-e "/# @LINUX_ARM64@/d" \
			"$root/packaging/safehell.rb.tmpl"
	fi
}

for mode in with without; do
	render_formula "$mode" >formula.rb

	if ruby -c formula.rb >/dev/null 2>&1; then
		ok "formula is valid Ruby ($mode arm64)"
	else
		no "formula is valid Ruby ($mode arm64)"
	fi

	# A renamed or dropped sed rule leaves the placeholder behind, which is
	# still valid Ruby and would ship a literal @VERSION@ to users.
	if grep -q '@[A-Z0-9_]*@' formula.rb; then
		no "formula has no unsubstituted placeholders ($mode arm64)"
	else
		ok "formula has no unsubstituted placeholders ($mode arm64)"
	fi

	# An empty substitution renders `sha256 ""`, which Homebrew accepts at
	# render time and only rejects mid-download on the user's machine.
	if grep 'sha256' formula.rb | grep -qvE '"[0-9a-f]{64}"'; then
		no "formula sha256 values are all 64-hex ($mode arm64)"
	else
		ok "formula sha256 values are all 64-hex ($mode arm64)"
	fi
done

# Dropping the stanza must remove it, not merely blank it out.
render_formula without >formula.rb
if grep -q 'aarch64-unknown-linux-gnu\|safehell-linux-aarch64' formula.rb; then
	no "formula omits the arm64 stanza entirely when the asset is missing"
else
	ok "formula omits the arm64 stanza entirely when the asset is missing"
fi

printf -- '---\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
