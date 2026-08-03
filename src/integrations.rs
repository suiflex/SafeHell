use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{Agent, config, executable};

pub fn install(agent: Agent) -> Result<()> {
    let executable = executable()?;
    register_mcp(agent, &executable)?;
    let (path, hook_agent) = match agent {
        Agent::Codex => (home()?.join(".codex/hooks.json"), "codex"),
        Agent::Claude => (home()?.join(".claude/settings.json"), "claude"),
    };
    install_hook(&path, &executable, hook_agent)?;
    println!("Installed SafeShell MCP and hook integration");
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
                    "permissionDecisionReason": "Direct SSH tools are blocked in this project. Use the SafeShell MCP tools so credentials stay brokered and each command is approved."
                }
            })
        );
    }
    Ok(())
}

fn register_mcp(agent: Agent, executable: &Path) -> Result<()> {
    let mut command = match agent {
        Agent::Codex => {
            let mut command = Command::new("codex");
            command.args(["mcp", "add", "safeshell", "--"]);
            command
        }
        Agent::Claude => {
            let mut command = Command::new("claude");
            command.args([
                "mcp",
                "add",
                "--transport",
                "stdio",
                "--scope",
                "user",
                "safeshell",
                "--",
            ]);
            command
        }
    };
    let status = command
        .arg(executable)
        .arg("mcp")
        .status()
        .context("agent CLI is not installed or not executable")?;
    if !status.success() {
        bail!("agent CLI failed to register the SafeShell MCP server");
    }
    Ok(())
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
        fs::copy(path, path.with_extension("json.safeshell.bak"))?;
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
            "echo ssh-keygen && safeshell exec prod -- uptime"
        ));
    }
}
