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
tool call, so it is kept short. `install` supports `codex`, `claude`, `cursor`,
`opencode`, `hermes`, `openclaw`, `antigravity`, `windsurf`, `copilot`, `cline`, and
`roo`. Omitting `--agent` opens a picker on a terminal and otherwise installs only for
the agents whose own configuration is detected — never for all of them. It removes stale
`safeshell` and `safehell` registrations — and any existing `shll` registration, so a
re-install also repairs a stale binary path — before adding it, so upgrading never leaves
two servers exposing the same tools (`src/integrations.rs`). Policy seeds in `AGENTS.md`,
`CLAUDE.md`, and `.gemini/GEMINI.md` are created only when absent and never rewritten;
the Cursor, Antigravity, and Roo rule files are SafeHell-owned and refreshed on
re-install.

Detection markers are always the agent's own configuration, never a file SafeHell wrote,
or a re-run would keep adding the target it created last time. `.agents` alone is not an
Antigravity marker for that reason: Codex, Cursor, and OpenCode all share
`.agents/skills`.

Where an agent keeps its MCP registry outside the repository, SafeHell writes it there
rather than seeding a policy that names tools nothing can reach: Hermes and Windsurf are
user-level in both scopes, and Cline and Roo Code live in the VS Code profile — skipped
with a notice when the extension has never run, since inventing that tree would leave
settings no editor loads. Copilot is repository-scoped in both directions, so `--global`
writes nothing for it.

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
| `integrations.rs` | per-agent MCP registration, policy seeds, and the `PreToolUse` hook |
| `update.rs` | self-update via the published installer |

`scripts/` installers, `packaging/` tap and bucket renderers, `npm/` the npm installer
package.

## CLI

`setup`, `init`, `update [--version]`, `server {add,list,remove}`, `serve [--yes]`,
`exec <alias> [--reason] [--max-lines] -- <cmd>`, `audit [--tail]`, `mcp`,
`install [--global] [--agent <codex,claude,cursor,opencode,hermes,openclaw,antigravity,windsurf,copilot,cline,roo>]`,
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
shellcheck -s sh scripts/install.sh tests/install_sh_test.sh tests/logo.sh
sh tests/install_sh_test.sh
node npm/install.js --selftest
sh tests/logo.sh --check
```

`tests/logo.sh --check` regenerates the picker's logo grid from
`assets/brand/logo-mark.svg` and fails if the committed grid drifted from it. It needs
macOS `qlmanage` and Pillow and skips itself without them; the grid is committed, so
building never needs either.

`AGENTS.md` is a symlink to this file. Edit this one.
