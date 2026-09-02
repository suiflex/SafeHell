<img src="https://raw.githubusercontent.com/suiflex/SafeHell/develop/assets/brand/logo-mark.svg" alt="" width="72" align="left">

# SafeHell

SafeHell is a local approval broker for SSH commands requested by AI coding agents. Credentials stay in an encrypted per-user vault, and every remote command is displayed in a separate foreground terminal before it can run.

> Pre-1.0: review the security model and limitations before using SafeHell on production systems.

## What it guarantees

- SafeHell never places stored passwords in `.safehell.toml`, CLI arguments, MCP schemas, or audit logs; literal password values are redacted from buffered agent-facing output.
- The vault is encrypted with an age X25519 identity stored in the operating-system credential store.
- Password credentials are bound to an exact host, port, and username.
- Every command needs a one-time `y` approval in `safehell serve`, except commands the project explicitly lists under `autoapprove.allow`.
- Commands matching `autoapprove.deny` are refused before any approval path, in both attended and unattended mode.
- SSH host keys are checked against SafeHell's own `known_hosts`; changed keys are rejected.
- Command output is buffered, bounded, and redacted before it is returned to the caller. Redaction covers bearer tokens, private-key blocks, `scheme://user:pass@` URLs, AWS access key ids, and `*_URL`/`*_TOKEN`/`*_SECRET`-style assignments, so container environment dumps lose their values as well as their secrets.

SafeHell does not sandbox the remote shell. Output redaction and agent hooks are defense-in-depth, not guarantees against every possible secret representation or bypass by another local process running as your user.

## Install

### macOS and Linux

```sh
curl -fsSL https://raw.githubusercontent.com/suiflex/SafeHell/develop/scripts/install.sh | sh
safehell setup
```

The script downloads the latest release binary for your platform, verifies it against the release `SHA256SUMS`, and installs it to `$HOME/.local/bin`. Set `SAFEHELL_VERSION` to pin a tag or `SAFEHELL_INSTALL_DIR` to change the destination. Read the script before piping it to a shell.

### Windows

```powershell
irm https://raw.githubusercontent.com/suiflex/SafeHell/develop/scripts/install.ps1 | iex
safehell setup
```

Installs to `%LOCALAPPDATA%\Programs\SafeHell\bin` and honours the same `SAFEHELL_VERSION` and `SAFEHELL_INSTALL_DIR` overrides. It verifies the download against `SHA256SUMS` just as the POSIX installer does.

### Homebrew

```sh
brew install suiflex/tap/safehell
```

### Scoop

```powershell
scoop bucket add suiflex https://github.com/suiflex/scoop-bucket
scoop install safehell
```

### npm

```sh
npm install -g @suiflex/safehell
```

Installing downloads and verifies the release binary for your platform. `npx @suiflex/safehell` works too.

### Cargo

```sh
cargo install safehell
```

Prebuilt binaries cover Linux, macOS, and Windows on both x86_64 and aarch64.

### Update

Update the installed binary without changing the vault, project configuration, or agent integrations:

```sh
safehell update
```

To install a specific release tag:

```sh
safehell update --version v0.2.0
```

The update downloads the official installer and verifies the binary against the release `SHA256SUMS` before replacing the installed executable.

### From source

Rust 1.85 or newer is required.

```sh
cargo install --path .
safehell setup
```

## Configure a project

Run these commands from the project root:

```sh
safehell init
safehell server add prod --host example.com --username deploy --auth password
safehell server add staging --host staging.example.com --username deploy --auth ssh-agent
safehell server list
```

Password entry requires a TTY. `safehell init` creates `.safehell.toml` and adds it to `.git/info/exclude` when the project is a Git repository.

Example config (never add secret fields):

```toml
version = 1
project_id = "1ef6c562-8c64-499e-a798-f74248d8ca04"

[limits]
timeout_seconds = 60
max_output_bytes = 1048576
approval_timeout_seconds = 120
max_commands_per_hour = 60
dedup_seconds = 10
approval_ttl_seconds = 300
max_transfer_bytes = 1048576

[servers.staging]
host = "staging.example.com"
port = 22
username = "deploy"

[servers.staging.auth]
type = "ssh-agent"

[servers.staging.autoapprove]
allow = ["docker logs *", "docker ps*", "docker inspect -f *", "systemctl status *", "df -h"]
deny = ["rm *", "dd *", "mkfs*", "docker rm *", "docker run *", "chown *", "* > *", "curl *| *sh"]
```

Limits are capped at 10 minutes and 10 MiB. Unknown config fields are rejected, so fields such as `password`, `secret`, and `private_key` cannot be smuggled into a server entry.

### Approval rules

Patterns use `*` as the only wildcard and are matched against the whole command string.

| Command matches | Result |
| --- | --- |
| `autoapprove.deny` | refused immediately, no prompt, never worth retrying |
| `autoapprove.allow` | runs without a prompt |
| neither | waits for `y` in the broker terminal, up to `approval_timeout_seconds` |

`allow = ["*"]` is rejected: automatic approval always has to name the commands it covers. An approval keeps covering the byte-identical command for `approval_ttl_seconds`, an identical command inside `dedup_seconds` is refused instead of run twice, and the broker stops executing once `max_commands_per_hour` is reached.

## Run

Keep the broker visible in its own terminal:

```sh
safehell serve
```

`safehell serve --yes` runs the broker unattended: allow-listed commands still
execute, and anything that would need a prompt is denied instead of waiting.
Every decision is recorded in the audit log.

Then request a command from the project:

```sh
safehell exec prod --reason "check deployment" -- "systemctl status my-app"
safehell exec prod --max-lines 60 -- "docker logs --tail 2000 engine-trade"
```

`exec` exits `3` when the broker refuses a command, separately from a transport
failure, so a wrapper can tell "denied" from "the broker is down".

## Long commands and file transfer

`execute` blocks until the command finishes. For anything slow, `start` returns a
`job_id` as soon as the request is approved, and `poll` reads what has been
produced so far:

```
start  -> job_id
poll   -> status: running, stdout_offset: 6, "line1"
poll   -> status: finished, exit_status: 0, "line2\nline3"
```

Pass the previous `stdout_offset` and `stderr_offset` back to `poll` to read only
what is new. A partial trailing line, and anything after an unterminated
`-----BEGIN` key header, is withheld until the job finishes, so redaction is never
applied to half a secret. The broker keeps the last 16 jobs and evicts finished
ones first.

A background job is still bounded by `timeout_seconds`, so raise that limit (up to
10 minutes) for work that needs it rather than expecting `start` to run forever.
`docker logs -f` and other endless commands are cut at the timeout.

`get_file` and `put_file` move a file through the same approval gate. What the
operator sees is the real remote command (`head -c N -- '<path>' | base64` or
`base64 -d > '<path>'`), so `autoapprove` patterns apply to transfers too. The
local side of a transfer must stay inside the project directory, and
`max_transfer_bytes` caps both directions. Transfers return only the path, size,
and SHA-256: content lands on disk instead of in the agent's context, and is not
redacted, so treat an approved transfer as handing over the file.

## Audit

Every decision, including refusals, is appended to a `0600` JSON Lines file:

```sh
safehell audit --tail 50
```

Each entry carries the timestamp, project id, alias, SHA-256 of the command, the
outcome (`approved`, `auto-approved`, `ttl-approved`, `denied`, `blocked`,
`duplicate`, `throttled`, `expired`, `unattended`, `executed`, `failed`,
`transferred`, `transfer-failed`), the duration, and the exit status. Command
text is hashed rather than stored.

Pass the remote command as one quoted shell string so its quoting and operators are preserved exactly.

The broker shows the project, endpoint, command, and reason. It decrypts a password only after approval. The first connection to an unknown host also shows its key fingerprint and asks whether to trust it.

## Codex, Claude Code, Cursor, OpenCode, and Antigravity

With the corresponding agent CLI installed (Codex and Claude Code only):

```sh
# Run from the project root; this is project-local by default.
safehell integrate install --agent codex
safehell integrate install --agent claude

# Comma-separated or repeated; omit --agent to install for every agent whose
# configuration is detected in the target directory.
safehell integrate install --agent codex,claude,cursor,opencode,antigravity
safehell integrate install

# Optional: install into the user's global agent configuration.
safehell integrate install --global --agent codex,claude
```

Every install registers the stdio MCP server as `shll` — the short name is
what an agent types on every tool call — and seeds the policy line that tells
the model to prefer SafeHell over direct SSH. Per agent:

|Agent|MCP registration|Policy seed|Guard hook|
|---|---|---|---|
|`codex`|`codex mcp add`|`AGENTS.md` (project) or `.codex/AGENTS.md` (global)|`.codex/hooks.json` `PreToolUse`|
|`claude`|`claude mcp add`|`CLAUDE.md` (project) or `.claude/CLAUDE.md` (global)|`.claude/settings.json` `PreToolUse`|
|`cursor`|`.cursor/mcp.json`|`.cursor/rules/safehell.mdc`|—|
|`opencode`|`opencode.json` `mcp` map|`AGENTS.md` (project) or `.config/opencode/AGENTS.md`|—|
|`antigravity`|`.agents/mcp_config.json` (project) or `~/.gemini/config/mcp_config.json`|`.agents/rules/safehell.md` (project) or `.gemini/GEMINI.md`|—|

Installing removes any earlier `safeshell` or `safehell` registration first
and replaces an existing `shll` registration, so upgrading never leaves two
servers exposing the same tools or a stale binary path behind. Policy seeds in
`AGENTS.md`, `CLAUDE.md`, and `.gemini/GEMINI.md` are created only when absent
and are never rewritten — the file may hold the user's own instructions.
Owned files (`.cursor/rules/safehell.mdc`, `.agents/rules/safehell.md`) are
refreshed on re-install. Existing JSON settings are backed up as
`*.json.safehell.bak` before modification. The guard blocks direct `ssh`,
`scp`, `sftp`, `sshpass`, and `rsync` calls. The MCP server exposes only:

- `list_servers`
- `execute` (write/destructive capable; still requires broker approval)
- `start` and `poll` for long commands
- `get_file` and `put_file`, capped by `max_transfer_bytes`

The hook does not rewrite commands and cannot prevent all bypasses. Agent policies should still restrict arbitrary shell execution where stronger isolation is required.

## Data and audit

SafeHell uses the platform user-data directory for:

- `vault.age`: encrypted credentials
- `known_hosts`: trusted SSH host keys
- `audit.jsonl`: timestamp, project ID, alias, command SHA-256, approval, duration, outcome, and exit status

The audit log never stores raw commands, stdout, stderr, or credentials. Losing the OS credential-store identity makes `vault.age` unrecoverable; back up both together if recovery is required.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

SafeHell is dual-licensed under Apache-2.0 or MIT.
