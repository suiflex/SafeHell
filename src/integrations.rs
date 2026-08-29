use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{Agent, config, executable};

/// Name the agent CLIs register the MCP server under. Deliberately short:
/// it is what a model types on every tool call, not a brand string.
const MCP_SERVER: &str = "shll";
/// Names earlier versions registered. Removed before re-registering so an
/// upgraded install does not expose the same tools twice.
const LEGACY_MCP_SERVERS: [&str; 2] = ["safeshell", "safehell"];

pub fn install(agent: Agent, global: bool) -> Result<()> {
    let executable = executable()?;
    let project = if global {
        None
    } else {
        Some(std::env::current_dir()?)
    };
    register_mcp(agent, &executable, global, project.as_deref())?;
    let (path, hook_agent) = (
        config_path(agent, global, project.as_deref())?,
        match agent {
            Agent::Codex => "codex",
            Agent::Claude => "claude",
        },
    );
    install_hook(&path, &executable, hook_agent)?;
    println!(
        "Installed SafeHell MCP and hook integration ({})",
        if global { "global" } else { "project" }
    );
    Ok(())
}

pub fn hook(_agent: Agent) -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let value: Value = serde_json::from_str(&input).context("invalid hook input")?;
    let command = value
        .pointer("/tool_input/command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    if config::discover(&cwd).is_ok() && contains_direct_ssh(command) {
        println!(
            "{}",
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "Direct SSH tools are blocked in this project. Use the SafeHell MCP tools so credentials stay brokered and each command is approved."
                }
            })
        );
    }
    Ok(())
}

/// Best effort: the CLI errors when the server was never registered, and that
/// is the common case, so a failure here must not abort the install.
fn unregister_legacy_mcp(agent: Agent, global: bool, project: Option<&Path>) {
    for name in LEGACY_MCP_SERVERS {
        let mut command = match agent {
            Agent::Codex => {
                let mut command = Command::new("codex");
                if let Some(project) = project {
                    command.current_dir(project);
                    command.env("CODEX_HOME", project.join(".codex"));
                }
                command.args(["mcp", "remove", name]);
                command
            }
            Agent::Claude => {
                let mut command = Command::new("claude");
                command.args([
                    "mcp",
                    "remove",
                    "--scope",
                    if global { "user" } else { "local" },
                ]);
                command.arg(name);
                command
            }
        };
        let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn register_mcp(
    agent: Agent,
    executable: &Path,
    global: bool,
    project: Option<&Path>,
) -> Result<()> {
    unregister_legacy_mcp(agent, global, project);
    let mut command = match agent {
        Agent::Codex => {
            let mut command = Command::new("codex");
            if let Some(project) = project {
                command.current_dir(project);
                command.env("CODEX_HOME", project.join(".codex"));
            }
            command.args(["mcp", "add", MCP_SERVER, "--"]);
            command
        }
        Agent::Claude => {
            let mut command = Command::new("claude");
            command.args(["mcp", "add", "--transport", "stdio"]);
            command.args(["--scope", if global { "user" } else { "local" }]);
            command.args([MCP_SERVER, "--"]);
            command
        }
    };
    let status = command
        .arg(executable)
        .arg("mcp")
        .status()
        .context("agent CLI is not installed or not executable")?;
    if !status.success() {
        bail!("agent CLI failed to register the SafeHell MCP server");
    }
    Ok(())
}

fn config_path(agent: Agent, global: bool, project: Option<&Path>) -> Result<PathBuf> {
    if !global {
        return Ok(project
            .context("project directory is required for local integration")?
            .join(match agent {
                Agent::Codex => ".codex/hooks.json",
                Agent::Claude => ".claude/settings.json",
            }));
    }
    Ok(match agent {
        Agent::Codex => home()?.join(".codex/hooks.json"),
        Agent::Claude => home()?.join(".claude/settings.json"),
    })
}

fn install_hook(path: &Path, executable: &Path, agent: &str) -> Result<()> {
    let mut root: Value = if path.exists() {
        serde_json::from_slice(&fs::read(path)?)
            .with_context(|| format!("invalid JSON in {}", path.display()))?
    } else {
        json!({})
    };
    let command = format!(
        "{} hook {agent}",
        shell_quote(&executable.to_string_lossy())
    );
    let entry = json!({
        "matcher": "Bash|Shell",
        "hooks": [{ "type": "command", "command": command }]
    });
    let object = root
        .as_object_mut()
        .context("agent settings root must be a JSON object")?;
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .context("hooks must be a JSON object")?;
    let pre = hooks.entry("PreToolUse").or_insert_with(|| json!([]));
    let pre = pre
        .as_array_mut()
        .context("hooks.PreToolUse must be an array")?;
    let exists = pre.iter().any(|item| {
        item.pointer("/hooks/0/command").and_then(Value::as_str)
            == entry.pointer("/hooks/0/command").and_then(Value::as_str)
    });
    if exists {
        return Ok(());
    }
    pre.push(entry);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        fs::copy(path, path.with_extension("json.safehell.bak"))?;
    }
    crate::config::atomic_write(path, serde_json::to_string_pretty(&root)?.as_bytes())
}

fn contains_direct_ssh(command: &str) -> bool {
    command
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ';' | '|' | '&' | '(' | ')')
        })
        .map(|token| token.trim_matches(['\'', '"']))
        .map(|token| token.rsplit(['/', '\\']).next().unwrap_or(token))
        .any(|token| matches!(token, "ssh" | "scp" | "sftp" | "sshpass" | "rsync"))
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn shell_quote(value: &str) -> String {
    format!("\"{value}\"")
}

fn home() -> Result<PathBuf> {
    std::env::home_dir().context("cannot determine home directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_direct_ssh_tools_without_false_substrings() {
        assert!(contains_direct_ssh("sudo ssh root@example.com uptime"));
        assert!(contains_direct_ssh("'ssh' root@example.com uptime"));
        assert!(contains_direct_ssh(
            r"C:\\Windows\\System32\\OpenSSH\\ssh root@example.com"
        ));
        assert!(contains_direct_ssh("rsync ./a host:/b"));
        assert!(!contains_direct_ssh(
            "echo ssh-keygen && safehell exec prod -- uptime"
        ));
    }
}
