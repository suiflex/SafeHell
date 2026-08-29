#!/usr/bin/env python3
"""Render the Scoop manifest for suiflex/scoop-bucket, printed to stdout.

Run from the repository root by .github/workflows/release-build.yml, with the
release's Windows archives already downloaded into the working directory.

A manifest is JSON, so it is built rather than string-templated: that makes an
unsubstituted placeholder or a dangling comma from an omitted optional stanza
impossible, and those are exactly the mistakes that stay invisible until a user
runs `scoop install`.

`TAG` and `REPO` come from the workflow environment. An architecture is included
only when its archive is present, because the arm64 build leg is best effort.
"""

import hashlib
import json
import os
import sys

REPO = os.environ.get("REPO", "suiflex/SafeHell")
TAG = os.environ["TAG"]
BASE = f"https://github.com/{REPO}/releases/download/{TAG}"

# scoop architecture name -> release asset
ARCHES = {
    "64bit": "safehell-windows-x86_64.zip",
    "arm64": "safehell-windows-aarch64.zip",
}


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    architecture = {}
    for arch, asset in ARCHES.items():
        if os.path.exists(asset):
            architecture[arch] = {"url": f"{BASE}/{asset}", "hash": sha256(asset)}

    # x86_64 is not a best-effort leg. Its absence means the build broke, and
    # publishing a manifest without it would leave scoop users on the old
    # version with no signal that anything went wrong.
    if "64bit" not in architecture:
        sys.exit("64bit archive is missing; refusing to render a manifest without it")

    json.dump(
        {
            "version": TAG.lstrip("v"),
            "description": "Approval-gated SSH broker for AI coding agents",
            "homepage": f"https://github.com/{REPO}",
            "license": "MIT",
            "architecture": architecture,
            "bin": "safehell.exe",
            "checkver": "github",
            "autoupdate": {
                "architecture": {
                    arch: {"url": f"https://github.com/{REPO}/releases/download/v$version/{asset}"}
                    for arch, asset in ARCHES.items()
                }
            },
        },
        sys.stdout,
        indent=2,
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
