#!/usr/bin/env python3
"""Add contributor attribution to the newest release block of CHANGELOG.md.

Runs after release-please updates the changelog on the pending release
branch. It rewrites only the topmost release block (`## [x.y.z]`, or
`## <product>: [x.y.z]` where a product prefix is used):

  * each bullet gains ` (@handle)` after the subject (before the commit
    link), resolved from the commit's GitHub author. Commits authored by a
    maintainer are left untouched so that work reads as plain maintainer
    work.
  * a `### Thanks` section lists every non-maintainer contributor in the
    block (returning contributors included).
  * a `### New Contributors` section welcomes anyone whose commit email is
    not present in git history before the previous release tag.

Re-runnable: a block that already carries a `### Thanks` section is returned
unchanged. release-please rebuilds the block from scratch on every run, so in
the workflow this always re-derives the attribution from a clean block.
"""

import os
import re
import subprocess
import sys

MAINTAINERS = {"mulhamna", "badrus123"}

_PREVTAG_RE = re.compile(r"/compare/(.+?)\.\.\.")
_BULLET_RE = re.compile(r"^(\* (?:\*\*[^:*]+:\*\* )?)(.*?)( \(\[[0-9a-f]+\]\([^)]*\)\))\s*$")
_SHA_RE = re.compile(r"\[([0-9a-f]+)\]")


def _split_top_block(text):
    """Return (before, top_block, after) splitting on `## ` release headings."""
    parts = re.split(r"(?m)(?=^## )", text)
    if not parts:
        return "", "", ""
    head = parts[0]
    sections = parts[1:]
    if not sections:
        return head, "", ""
    return head, sections[0], "".join(sections[1:])


def gh_author(sha):
    """(login, name, email) for a commit sha, via `gh api`. All may be None."""
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    env = dict(os.environ)
    if token:
        env["GH_TOKEN"] = token
    proc = subprocess.run(
        [
            "gh",
            "api",
            f"repos/suiflex/SafeHell/commits/{sha}",
            "--jq",
            r"[(.author.login // \"\"), (.commit.author.name // \"\"), (.commit.author.email // \"\")] | @tsv",
        ],
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )
    if proc.returncode != 0:
        return None, None, None
    cols = proc.stdout.strip().split("\t")
    while len(cols) < 3:
        cols.append("")
    return (cols[0] or None, cols[1] or None, cols[2] or None)


def prev_emails(prevtag):
    """Author emails in git history up to and including `prevtag`."""
    proc = subprocess.run(
        ["git", "log", prevtag, "--format=%ae"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        return set()
    return {line.strip() for line in proc.stdout.splitlines() if line.strip()}


def _token(login, name):
    """Display token for a contributor: `@login` when known, else the name."""
    if login:
        return f"@{login}"
    return name or "an unknown contributor"


def attribute(text, resolve=gh_author, history=prev_emails, product="SafeHell"):
    before, block, after = _split_top_block(text)
    if not block or "### Thanks" in block:
        return text

    match = _PREVTAG_RE.search(block)
    prevtag = match.group(1) if match else None
    seen_emails = history(prevtag) if prevtag else set()

    contributors = {}
    new_contributors = {}

    lines = []
    for line in block.splitlines():
        bullet_match = _BULLET_RE.match(line)
        if not bullet_match:
            lines.append(line)
            continue

        prefix, subject, link = bullet_match.groups()
        sha_match = _SHA_RE.search(link)
        if not sha_match:
            lines.append(line)
            continue

        sha = sha_match.group(1)
        login, name, email = resolve(sha)
        if login in MAINTAINERS:
            lines.append(line)
            continue

        if not login and not name:
            lines.append(line)
            continue

        tok = _token(login, name)
        contributors[tok] = None
        if email and email not in seen_emails:
            new_contributors[tok] = None

        lines.append(f"{prefix}{subject} ({tok}){link}")

    if not contributors:
        return text

    sorted_contributors = sorted(contributors)
    thanks = ["", "### Thanks", ""]
    for tok in sorted_contributors:
        thanks.append(f"* {tok}")

    if new_contributors:
        sorted_new_contributors = sorted(new_contributors)
        thanks.extend(["", "### New Contributors", ""])
        for tok in sorted_new_contributors:
            thanks.append(f"* {tok} made their first contribution")

    rebuilt = before + "\n".join(lines).rstrip() + "\n" + "\n".join(thanks) + "\n\n" + after
    return rebuilt.rstrip("\n") + "\n"

def main():
    dst = sys.argv[1] if len(sys.argv) > 1 else "CHANGELOG.md"
    try:
        with open(dst, "r", encoding="utf-8") as file:
            src = file.read()
    except FileNotFoundError:
        print(f"attribute_changelog: {dst} not found", file=sys.stderr)
        sys.exit(1)

    out = attribute(src)
    with open(dst, "w", encoding="utf-8") as file:
        file.write(out)
    print(f"attributed contributors in {dst}")


if __name__ == "__main__":
    main()
