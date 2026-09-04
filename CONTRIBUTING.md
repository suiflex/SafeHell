# Contributing to SafeHell

Thanks for considering a contribution. SafeHell is an approval-gated SSH broker
for AI coding agents: it holds credentials the agent never sees, and puts a
person in front of every remote command. That shapes what a good change looks
like here, so this guide is worth a skim before you start.

## Quick links

- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md) — never file a vulnerability as a public issue
- [Issue forms](https://github.com/suiflex/SafeHell/issues/new/choose)
- [Architecture notes](CLAUDE.md) — module map and the invariants that break quietly

## How to contribute

- **Found a bug?** Use the bug form. It asks for your platform, install method,
  and `safehell --version` up front, which saves a round trip.
- **Want a feature?** Use the feature form. It asks what the change does to the
  protection boundary; "nothing I can see" is a fine answer, but the question
  should reach you before it reaches a reviewer.
- **Found a vulnerability?** Do not open an issue. Follow
  [SECURITY.md](SECURITY.md).

Small fixes are welcome without discussion. For anything that changes the
approval model, the vault format, or the MCP surface, open an issue first so
the design conversation happens before you write the code.

## Getting started

Rust 1.85 or newer.

```sh
git clone https://github.com/suiflex/SafeHell.git
cd SafeHell
cargo build --release
```

To try it end to end you need a host you can reach over SSH:

```sh
./target/release/safehell setup          # create the vault
./target/release/safehell init           # write .safehell.toml
./target/release/safehell server add     # register a host
./target/release/safehell serve          # approval broker, keep it running
./target/release/safehell exec <alias> -- uptime   # in another terminal
```

`serve` is the human-facing half. Nothing runs remotely without a `y` in that
terminal, except commands the project lists under `autoapprove.allow`.

## Build, lint, test

Run what your change touches:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets

sh tests/install_sh_test.sh      # scripts/install.sh
node npm/install.js --selftest   # npm/
sh packaging/test_render.sh      # packaging/
shellcheck -s sh scripts/install.sh tests/install_sh_test.sh packaging/test_render.sh
```

### What CI covers

CI is split by concern, and each workflow only runs when files it actually
tests change. A docs-only change runs no build at all — that is intended, not a
CI failure.

| Workflow | Runs when you touch | What it does |
|---|---|---|
| `ci-rust` | `src/`, `tests/fixtures/`, `Cargo.*` | fmt, clippy, test on Linux, macOS, Windows |
| `ci-scripts` | `scripts/`, `tests/install_sh_test.sh` | shellcheck and the installer checks on Linux and macOS, plus a PowerShell parse on Windows |
| `ci-npm` | `npm/` | the installer selftest |
| `ci-packaging` | `packaging/` | shellcheck, and the formula and manifest renderers |

The installer checks run on both Linux and macOS on purpose: `sha256_of` picks
`sha256sum` where it exists and `shasum` otherwise, so a single platform only
exercises half of the checksum verification.

## Things that break quietly

These are the couplings that no single file protects. If you touch one side,
check the others.

- **Release asset names** appear in `.github/workflows/release-build.yml`,
  `scripts/install.sh`, `tests/install_sh_test.sh`, and `npm/install.js`. A
  mismatch is invisible until someone installs a published release.
- **Every download path verifies against `SHA256SUMS`** — the shell installer,
  the PowerShell installer, and the npm postinstall. No install path may be
  weaker than the others.
- **`.github/labeler.yml` mirrors the `paths` filters** in `ci-*.yml`. Nothing
  checks that they agree.
- **Secrets never reach** `.safehell.toml`, CLI arguments, MCP schemas, or the
  audit log. `security.rs` redacts agent-facing output; `config.rs` rejects
  plaintext secret fields.

## Commit conventions

Commits follow [Conventional Commits](https://www.conventionalcommits.org/).
Release Please reads them to decide the next version and to build the
changelog, so the prefix matters.

| Prefix | Effect |
|---|---|
| `feat:` | minor bump, listed under **Features** |
| `fix:` | patch bump, listed under **Bug Fixes** |
| `perf:` | patch bump, listed under **Performance Improvements** |
| `revert:` | patch bump, listed under **Reverts** |
| `docs:` `chore:` `refactor:` `test:` `build:` `ci:` `style:` | no release entry |

Mark a breaking change with `!` or a `BREAKING CHANGE:` footer. While the
project is pre-1.0 that produces a minor bump, not a major one.

Keep the subject at 72 characters or fewer, in the imperative, with no trailing
period. Use the body to explain **why**; the diff already shows what.

## Branching and pull requests

- Branch off `develop`, named for the leading commit type: `feat/…`, `fix/…`,
  `refactor/…`, `chore/…`, `docs/…`.
- Fill out the pull request template.
- Keep each pull request to one logical change. Smaller is easier to review and
  safer to revert.
- Fill the test plan honestly. Tick what you ran and leave the rest unchecked —
  an unchecked box is information, a wrongly ticked one wastes a review cycle.

### AI-assisted pull requests

AI-assisted pull requests are welcome, and plenty of good contributions start
that way. You do not need to label them. Two things are asked:

- **Evidence.** Say which commands you ran and what they printed. The test plan
  in the template is the place for it.
- **Understand what you are submitting.** If a reviewer asks why a line is
  there, you should be able to answer. The pull requests that stall are the
  ones whose author cannot.

## License

SafeHell is licensed under [Apache 2.0](LICENSE). By contributing, you agree
that your contribution is licensed under the same terms. There is no CLA to
sign.
