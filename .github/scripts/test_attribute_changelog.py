#!/usr/bin/env python3
"""Self-check for attribute_changelog.py. Run: python3 test_attribute_changelog.py"""

from attribute_changelog import attribute

# sha -> (login, name, email)
AUTHORS = {
    "aaa": ("mulhamna", "Muhammad Ulham", "ulham@example.com"),
    "bbb": ("wahyuakbarwibowo", "Wahyu Akbar", "wahyu@example.com"),
    "ccc": (None, "External Contributor", "external@example.com"),
    "ddd": ("ekacahya21", "Eka Cahya", "eka@example.com"),
    "eee": ("mulhamna", "Muhammad Ulham", "ulham@example.com"),
}

ROOT = """# Changelog

## [0.4.1](https://github.com/suiflex/SafeHell/compare/v0.4.0...v0.4.1) (2026-09-09)

### Features

* **tray:** implement desktop system tray ([bbb](https://github.com/suiflex/SafeHell/commit/bbb))
* **cli:** enhance server commands ([aaa](https://github.com/suiflex/SafeHell/commit/aaa))
* **core:** add missing feature from external ([ccc](https://github.com/suiflex/SafeHell/commit/ccc))

### Bug Fixes

* **broker:** resolve race in background job ([ddd](https://github.com/suiflex/SafeHell/commit/ddd))

## [0.4.0](https://github.com/suiflex/SafeHell/compare/v0.3.0...v0.4.0) (2026-09-04)

### Features

* **init:** initial release ([eee](https://github.com/suiflex/SafeHell/commit/eee))
"""


def _resolve(sha):
    return AUTHORS[sha]


def test_appends_handle_before_the_commit_link():
    out = attribute(
        ROOT, resolve=_resolve, history=lambda t: {"wahyu@example.com", "eka@example.com"}
    )
    assert "* **tray:** implement desktop system tray (@wahyuakbarwibowo) ([bbb]" in out, out
    assert "* **core:** add missing feature from external (External Contributor) ([ccc]" in out, out


def test_maintainer_commits_are_left_untouched():
    out = attribute(
        ROOT, resolve=_resolve, history=lambda t: {"wahyu@example.com", "eka@example.com"}
    )
    assert "* **cli:** enhance server commands ([aaa]" in out, out
    assert "@mulhamna" not in out


def test_thanks_section_lists_non_maintainers_deduped():
    out = attribute(
        ROOT, resolve=_resolve, history=lambda t: {"wahyu@example.com", "eka@example.com"}
    )
    top = out.split("## [0.4.0]")[0]
    assert "### Thanks" in top
    assert "* @wahyuakbarwibowo\n" in top
    assert "* @ekacahya21\n" in top
    assert "* External Contributor\n" in top
    assert "* @mulhamna" not in top


def test_new_contributors_are_those_absent_from_history():
    out = attribute(
        ROOT, resolve=_resolve, history=lambda t: {"wahyu@example.com"}
    )
    top = out.split("## [0.4.0]")[0]
    assert "### New Contributors" in top
    assert "* @ekacahya21 made their first contribution" in top
    assert "* External Contributor made their first contribution" in top
    assert "@wahyuakbarwibowo made their first contribution" not in top


def test_is_idempotent():
    once = attribute(ROOT, resolve=_resolve, history=lambda t: {"wahyu@example.com"})
    twice = attribute(once, resolve=_resolve, history=lambda t: {"wahyu@example.com"})
    assert once == twice


def test_maintainer_only_release_gets_no_sections():
    root = """# Changelog

## [0.4.1](https://github.com/suiflex/SafeHell/compare/v0.4.0...v0.4.1) (2026-09-09)

### Features

* **cli:** maintainer only ([aaa](https://github.com/suiflex/SafeHell/commit/aaa))
"""
    out = attribute(root, resolve=_resolve, history=lambda t: set())
    assert "### Thanks" not in out


if __name__ == "__main__":
    cases = sorted((name, fn) for name, fn in globals().items() if name.startswith("test_"))
    for name, fn in cases:
        fn()
        print(f"ok   {name}")
    print("---")
    print(f"{len(cases)} passed, 0 failed")
