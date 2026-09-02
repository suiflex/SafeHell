# SafeHell

Local approval broker for SSH commands requested by AI coding agents. Credentials live in
an encrypted per-user vault; every remote command is shown in a separate foreground
terminal before it runs.

For code changes, use `/forgeguard-engineering`.

## Naming

The project was renamed from `safeshell` (that name is taken on crates.io). Nothing should
reintroduce it.

| Thing | Name |
|---|---|
| Crate and binary | `safehell` |
| npm package | `@suiflex/safehell` |
| MCP server registration | `shll` |
| Project config file | `.safehell.toml` |
| Keyring service | `dev.safehell.master` |
| Data dir | `ProjectDirs::from("dev", "SafeHell", "SafeHell")` |
| Release assets | `safehell-{linux,macos,windows}-{x86_64,aarch64}` |

The MCP server is `shll` and not the product name on purpose: a model types it on every
tool call, so it is kept short. `integrate install` supports `codex`, `claude`, `cursor`,
`opencode`, and `antigravity`; omitting `--agent` installs for every agent whose
configuration is detected in the target directory. It removes stale `safeshell` and
`safehell` registrations — and any existing `shll` registration, so a re-install also
repairs a stale binary path — before adding it, so upgrading never leaves two servers
exposing the same tools (`src/integrations.rs`). Policy seeds in `AGENTS.md`, `CLAUDE.md`,
and `.gemini/GEMINI.md` are created only when absent and never rewritten; the Cursor and
Antigravity rule files are SafeHell-owned and refreshed on re-install.

## Layout

`src/`

| Module | Responsibility |
|---|---|
| `main.rs` | CLI surface and dispatch |
| `config.rs` | `.safehell.toml` parsing, validation, atomic writes |
| `vault.rs` | age-encrypted vault, keyring identity, data dir, `known_hosts` |
| `broker.rs` | the approval loop behind `safehell serve` |
| `ipc.rs` | broker socket / named pipe transport |
| `policy.rs` | allow and deny lists, budgets, approval TTL and dedup |
| `remote.rs` | SSH execution, output bounding, file transfer |
| `security.rs` | secret redaction of agent-facing output |
| `mcp.rs` | the MCP server and its tools |
| `integrations.rs` | Codex / Claude Code MCP registration and `PreToolUse` hook |
| `update.rs` | self-update via the published installer |

`scripts/` installers, `packaging/` tap and bucket renderers, `npm/` the npm installer
package.

## CLI

`setup`, `init`, `update [--version]`, `server {add,list,remove}`, `serve [--yes]`,
`exec <alias> [--reason] [--max-lines] -- <cmd>`, `audit [--tail]`, `mcp`,
`integrate install [--global] [--agent <codex,claude,cursor,opencode,antigravity>]`,
and the hidden `hook <agent>`.

## Invariants

**Release asset names are duplicated in three places** and must change together:
`.github/workflows/release-build.yml` (the build matrix), `scripts/install.sh`
(`detect_asset`), and `tests/install_sh_test.sh` (assertions). A mismatch is invisible
until a user runs the installer against a published release. `npm/install.js` has a
fourth copy, guarded by its own `--selftest`.

**Anything that downloads a binary verifies it against the release `SHA256SUMS`** before
executing it — `scripts/install.sh`, `scripts/install.ps1`, and `npm/install.js` all do.
No install path may be weaker than the others.

**Secrets never reach `.safehell.toml`, CLI arguments, MCP schemas, or audit logs.**
`security.rs` redacts buffered output; `config.rs` rejects plaintext secret fields.

## Release

Release Please owns versions. Do not bump `Cargo.toml` or tag by hand.

1. Merge conventional commits to `develop`.
2. Run the `release-please` workflow to open or refresh the release PR.
3. Merging that PR tags `vX.Y.Z` and titles the GitHub Release `SafeHell vX.Y.Z`.
4. The tag triggers `release-build`, which builds six targets and fans out to GitHub
   release assets, crates.io, npm, `suiflex/homebrew-tap`, and `suiflex/scoop-bucket`.

Tags stay bare `vX.Y.Z` so the build trigger matches; only the Release title is prefixed.
No `-alpha` or other prerelease suffixes.

The arm64 Linux and Windows build legs are best effort. They cannot fail the release, and
the formula and manifest renderers omit an architecture whose asset is missing rather than
publishing a URL that 404s.

Required secrets: `RELEASE_PLEASE_TOKEN`, `TAP_PUBLISH_TOKEN` (write on both tap repos),
`CARGO_REGISTRY_TOKEN`. npm uses OIDC trusted publishing from the `Release` environment
rather than a token.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
shellcheck -s sh scripts/install.sh tests/install_sh_test.sh
sh tests/install_sh_test.sh
node npm/install.js --selftest
```

`AGENTS.md` is a symlink to this file. Edit this one.
