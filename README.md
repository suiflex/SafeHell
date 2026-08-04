# SafeShell

SafeShell is a local approval broker for SSH commands requested by AI coding agents. Credentials stay in an encrypted per-user vault, and every remote command is displayed in a separate foreground terminal before it can run.

> Public alpha: review the security model and limitations before using SafeShell on production systems.

## What it guarantees

- SafeShell never places stored passwords in `.safeshell.toml`, CLI arguments, MCP schemas, or audit logs; literal password values are redacted from buffered agent-facing output.
- The vault is encrypted with an age X25519 identity stored in the operating-system credential store.
- Password credentials are bound to an exact host, port, and username.
- Every command needs a one-time `y` approval in `safeshell serve`.
- SSH host keys are checked against SafeShell's own `known_hosts`; changed keys are rejected.
- Command output is buffered, bounded, and redacted before it is returned to the caller.

SafeShell does not sandbox the remote shell. Output redaction and agent hooks are defense-in-depth, not guarantees against every possible secret representation or bypass by another local process running as your user.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/badrus123/SafeShell/develop/install.sh | sh
safeshell setup
```

The script downloads the latest release binary for your platform, verifies it against the release `SHA256SUMS`, and installs it to `$HOME/.local/bin`. Set `SAFESHELL_VERSION` to pin a tag or `SAFESHELL_INSTALL_DIR` to change the destination. Read the script before piping it to a shell.

Prebuilt binaries cover Linux and macOS on x86_64 and aarch64, and Windows on x86_64. Windows users should download `safeshell-windows-x86_64.exe` from the [releases page](https://github.com/badrus123/SafeShell/releases) directly.

### Update

Update the installed binary without changing the vault, project configuration, or agent integrations:

```sh
safeshell update
```

To install a specific release tag:

```sh
safeshell update --version v0.1.0-alpha.1
```

The update downloads the official installer and verifies the binary against the release `SHA256SUMS` before replacing the installed executable.

### From source

Rust 1.85 or newer is required.

```sh
cargo install --path .
safeshell setup
```

## Configure a project

Run these commands from the project root:

```sh
safeshell init
safeshell server add prod --host example.com --username deploy --auth password
safeshell server add staging --host staging.example.com --username deploy --auth ssh-agent
safeshell server list
```

Password entry requires a TTY. `safeshell init` creates `.safeshell.toml` and adds it to `.git/info/exclude` when the project is a Git repository.

Example config (never add secret fields):

```toml
version = 1
project_id = "1ef6c562-8c64-499e-a798-f74248d8ca04"

[limits]
timeout_seconds = 60
max_output_bytes = 1048576

[servers.staging]
host = "staging.example.com"
port = 22
username = "deploy"

[servers.staging.auth]
type = "ssh-agent"
```

Limits are capped at 10 minutes and 10 MiB. Unknown config fields are rejected, so fields such as `password`, `secret`, and `private_key` cannot be smuggled into a server entry.

## Run

Keep the broker visible in its own terminal:

```sh
safeshell serve
```

Then request a command from the project:

```sh
safeshell exec prod --reason "check deployment" -- "systemctl status my-app"
```

Pass the remote command as one quoted shell string so its quoting and operators are preserved exactly.

The broker shows the project, endpoint, command, and reason. It decrypts a password only after approval. The first connection to an unknown host also shows its key fingerprint and asks whether to trust it.

Commands are non-interactive: no remote PTY, file transfer, port forwarding, or private-key-file mode is provided in this alpha.

## Codex and Claude Code

With the corresponding agent CLI installed:

```sh
# Run from the project root; this is project-local by default.
safeshell integrate install codex
safeshell integrate install claude

# Optional: install for every project.
safeshell integrate install --global codex
safeshell integrate install --global claude
```

By default, this registers the stdio MCP server and installs the `PreToolUse` guard in the current project. Use `--global` explicitly to install it for every project. The guard blocks direct `ssh`, `scp`, `sftp`, `sshpass`, and `rsync` calls. Existing JSON settings are backed up before modification. The MCP server exposes only:

- `list_servers`
- `execute` (write/destructive capable; still requires broker approval)

The hook does not rewrite commands and cannot prevent all bypasses. Agent policies should still restrict arbitrary shell execution where stronger isolation is required.

## Data and audit

SafeShell uses the platform user-data directory for:

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

SafeShell is dual-licensed under Apache-2.0 or MIT.
