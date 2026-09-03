use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::{Agent, config, executable};

/// How `install` chooses its targets: exactly these agents, or whichever ones
/// leave their own configuration under the install root.
pub enum AgentSelection {
    Explicit(Vec<Agent>),
    Detected,
}

/// Name the agent CLIs register the MCP server under. Deliberately short:
/// it is what a model types on every tool call, not a brand string.
const MCP_SERVER: &str = "shll";
/// Names earlier versions registered. Removed before re-registering so an
/// upgraded install does not expose the same tools twice.
const LEGACY_MCP_SERVERS: [&str; 2] = ["safeshell", "safehell"];

const POLICY_SEED: &str = "# SafeHell

For remote commands, use the `shll` MCP tools instead of direct `ssh`, `scp`, \
`sftp`, `sshpass`, or `rsync`; every call is approved in the SafeHell broker \
terminal.
";

const CURSOR_POLICY_SEED: &str = "---\nalwaysApply: true\n---\n\nFor remote \
commands, use the `shll` MCP tools instead of direct `ssh`, `scp`, `sftp`, \
`sshpass`, or `rsync`; every call is approved in the SafeHell broker terminal.\n";

/// Paths that show an agent is actually used here, so a bare `install`
/// configures those alone instead of writing every integration into every
/// repository. A marker is always the agent's own configuration, never
/// something SafeHell writes: otherwise every re-run would silently add a
/// target. `.agents` alone is not an Antigravity marker for the same reason —
/// Codex, Cursor, and OpenCode all share `.agents/skills`.
const PROJECT_AGENT_MARKERS: &[(Agent, &[&str])] = &[
    (Agent::Codex, &[".codex"]),
    (Agent::Claude, &[".claude"]),
    (Agent::Cursor, &[".cursor", ".cursorrules"]),
    (Agent::Opencode, &[".opencode", "opencode.json"]),
    (Agent::Hermes, &[".hermes"]),
    (Agent::Openclaw, &[".openclaw", "openclaw.json"]),
    (
        Agent::Antigravity,
        &[".agents/rules", ".agents/hooks.json", ".agent/rules"],
    ),
    (Agent::Windsurf, &[".windsurf", ".devin", ".windsurfrules"]),
    (
        Agent::Copilot,
        &[".github/copilot-instructions.md", ".github/instructions"],
    ),
    (Agent::Cline, &[".clinerules"]),
    (Agent::Roo, &[".roo", ".roorules"]),
];

/// Home-directory equivalents. Copilot is absent because its instructions are
/// repository-scoped, and Cline because the `~/.agents` directory it reads is
/// also created by a global Codex or Cursor install, which would make it
/// detect itself on the next run.
const GLOBAL_AGENT_MARKERS: &[(Agent, &[&str])] = &[
    (Agent::Codex, &[".codex"]),
    (Agent::Claude, &[".claude"]),
    (Agent::Cursor, &[".cursor"]),
    (Agent::Opencode, &[".config/opencode"]),
    (Agent::Hermes, &[".hermes"]),
    (Agent::Openclaw, &[".openclaw"]),
    (Agent::Antigravity, &[".gemini"]),
    (Agent::Windsurf, &[".codeium/windsurf", ".devin"]),
    (Agent::Roo, &[".roo"]),
];

pub fn install(selection: AgentSelection, global: bool) -> Result<()> {
    let executable = executable()?;
    let project = if global {
        None
    } else {
        Some(std::env::current_dir()?)
    };
    let agents = match selection {
        AgentSelection::Explicit(agents) => agents,
        AgentSelection::Detected => {
            let detection_root = if global {
                home()?
            } else {
                project
                    .as_deref()
                    .context("project directory is required for local integration")?
                    .to_path_buf()
            };
            let detected = detect_installed_agents(&detection_root, global);
            if detected.is_empty() {
                bail!(
                    "no supported agent configuration found under {}; pass --agent \
                     with one or more of {} to choose explicitly",
                    detection_root.display(),
                    crate::AGENTS
                        .iter()
                        .map(|agent| agent.slug())
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            detected
        }
    };
    let mut written = Vec::new();
    let home = home()?;
    let root = agent_root(global, project.as_deref())?;
    for agent in agents {
        install_agent(
            agent,
            &root,
            &home,
            &executable,
            global,
            project.as_deref(),
            &mut written,
        )?;
    }
    println!(
        "Installed SafeHell MCP integration ({})",
        if global { "global" } else { "project" }
    );
    for path in &written {
        println!("  {path}");
    }
    Ok(())
}

pub fn hook(_agent: &str) -> Result<()> {
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

fn agent_root(global: bool, project: Option<&Path>) -> Result<PathBuf> {
    if global {
        home()
    } else {
        Ok(project
            .context("project directory is required for local integration")?
            .to_path_buf())
    }
}

fn install_agent(
    agent: Agent,
    root: &Path,
    home: &Path,
    executable: &Path,
    global: bool,
    project: Option<&Path>,
    written: &mut Vec<String>,
) -> Result<()> {
    match agent {
        Agent::Codex => {
            register_mcp_cli(agent, executable, global, project)?;
            install_hook(&root.join(".codex/hooks.json"), executable, "codex")?;
            seed_user_policy(
                root,
                if global {
                    ".codex/AGENTS.md"
                } else {
                    "AGENTS.md"
                },
                written,
            )?;
        }
        Agent::Claude => {
            register_mcp_cli(agent, executable, global, project)?;
            install_hook(&root.join(".claude/settings.json"), executable, "claude")?;
            seed_user_policy(
                root,
                if global {
                    ".claude/CLAUDE.md"
                } else {
                    "CLAUDE.md"
                },
                written,
            )?;
        }
        Agent::Cursor => {
            write_mcp_servers_json(&root.join(".cursor/mcp.json"), executable, written)?;
            seed_owned_policy(
                root,
                ".cursor/rules/safehell.mdc",
                CURSOR_POLICY_SEED,
                written,
            )?;
        }
        Agent::Opencode => {
            let config = if global {
                root.join(".config/opencode/opencode.json")
            } else {
                root.join("opencode.json")
            };
            write_opencode_mcp(&config, executable, written)?;
            seed_user_policy(
                root,
                if global {
                    ".config/opencode/AGENTS.md"
                } else {
                    "AGENTS.md"
                },
                written,
            )?;
        }
        Agent::Antigravity => {
            if global {
                write_mcp_servers_json(
                    &root.join(".gemini/config/mcp_config.json"),
                    executable,
                    written,
                )?;
                seed_user_policy(root, ".gemini/GEMINI.md", written)?;
            } else {
                write_mcp_servers_json(&root.join(".agents/mcp_config.json"), executable, written)?;
                seed_owned_policy(root, ".agents/rules/safehell.md", POLICY_SEED, written)?;
            }
        }
        Agent::Hermes => {
            // Hermes keeps servers in `~/.hermes/config.yaml`, which SafeHell
            // will not hand-edit: rewriting YAML without a parser would risk
            // the rest of the user's configuration. Its own CLI owns the file.
            register_mcp_cli(agent, executable, global, project)?;
            if !global {
                seed_user_policy(root, "AGENTS.md", written)?;
            }
        }
        Agent::Openclaw => {
            let config = if global {
                root.join(".openclaw/openclaw.json")
            } else {
                root.join("openclaw.json")
            };
            write_openclaw_mcp(&config, executable, written)?;
            if !global {
                seed_user_policy(root, "AGENTS.md", written)?;
            }
        }
        Agent::Windsurf => {
            // Windsurf reads MCP servers from the user directory only, so a
            // project install still registers there; anything else would seed
            // a policy naming tools no agent can reach.
            write_mcp_servers_json(
                &home.join(".codeium/windsurf/mcp_config.json"),
                executable,
                written,
            )?;
            if global {
                seed_user_policy(root, ".codeium/windsurf/memories/global_rules.md", written)?;
            } else {
                seed_user_policy(root, "AGENTS.md", written)?;
            }
        }
        Agent::Copilot => {
            // Copilot instructions and `.vscode/mcp.json` are both
            // repository-scoped; there is no documented user-level equivalent
            // to guess at.
            if global {
                eprintln!(
                    "copilot has no documented user-level configuration; run without --global"
                );
            } else {
                write_vscode_mcp(&root.join(".vscode/mcp.json"), executable, written)?;
                seed_user_policy(root, ".github/copilot-instructions.md", written)?;
            }
        }
        Agent::Cline | Agent::Roo => {
            // Both store MCP servers in the VS Code profile rather than the
            // repository, so the registration is user-level in either scope.
            match vscode_mcp_settings(home, agent) {
                Some(path) => write_mcp_servers_json(&path, executable, written)?,
                None => eprintln!(
                    "{} is not installed in this VS Code profile; skipped its MCP registration",
                    agent.slug()
                ),
            }
            match (agent, global) {
                (Agent::Cline, true) => seed_user_policy(root, ".agents/AGENTS.md", written)?,
                (Agent::Roo, true) => {
                    seed_owned_policy(root, ".roo/rules/safehell.md", POLICY_SEED, written)?
                }
                _ => seed_user_policy(root, "AGENTS.md", written)?,
            }
        }
    }
    Ok(())
}

/// Directory VS Code keeps per-extension state in. Cline and Roo Code both
/// store their MCP settings there rather than in the repository.
#[cfg(target_os = "macos")]
fn vscode_user_dir(home: &Path) -> Option<PathBuf> {
    Some(home.join("Library/Application Support/Code/User"))
}

#[cfg(windows)]
fn vscode_user_dir(_home: &Path) -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("Code/User"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn vscode_user_dir(home: &Path) -> Option<PathBuf> {
    Some(home.join(".config/Code/User"))
}

/// `None` when the extension has never run here. The directory is not created:
/// inventing a VS Code profile tree would leave settings no editor reads.
fn vscode_mcp_settings(home: &Path, agent: Agent) -> Option<PathBuf> {
    let extension = match agent {
        Agent::Cline => "saoudrizwan.claude-dev",
        Agent::Roo => "rooveterinaryinc.roo-cline",
        _ => return None,
    };
    let directory = vscode_user_dir(home)?.join("globalStorage").join(extension);
    directory
        .is_dir()
        .then(|| directory.join("settings/cline_mcp_settings.json"))
}

/// Report which agents leave configuration under `root`, so a bare `install`
/// configures those alone.
pub fn detect_installed_agents(root: &Path, global: bool) -> Vec<Agent> {
    let markers = if global {
        GLOBAL_AGENT_MARKERS
    } else {
        PROJECT_AGENT_MARKERS
    };
    markers
        .iter()
        .filter(|(_, paths)| paths.iter().any(|path| root.join(path).exists()))
        .map(|(agent, _)| *agent)
        .collect()
}

/// Seed a policy line in a file the user owns. Created only when absent and
/// never rewritten afterwards: the file may hold the user's own instructions.
fn seed_user_policy(root: &Path, relative: &str, written: &mut Vec<String>) -> Result<()> {
    seed_policy(root, relative, POLICY_SEED, false, written)
}

/// Write a policy file SafeHell owns, so upgrades refresh it in place.
fn seed_owned_policy(
    root: &Path,
    relative: &str,
    content: &str,
    written: &mut Vec<String>,
) -> Result<()> {
    seed_policy(root, relative, content, true, written)
}

fn seed_policy(
    root: &Path,
    relative: &str,
    content: &str,
    owned: bool,
    written: &mut Vec<String>,
) -> Result<()> {
    // The relative path comes from the compile-time per-agent tables, but a
    // caller could pass `../`; keep every write inside the target root.
    if relative.starts_with('/') || relative.split(['/', '\\']).any(|part| part == "..") {
        bail!("policy path escapes the target root: {relative}");
    }
    let path = root.join(relative);
    // A symlink belongs to whoever made it; writing through one would edit
    // the file at the other end under the wrong name (AGENTS.md -> CLAUDE.md
    // is a common layout).
    if fs::symlink_metadata(&path).is_ok_and(|entry| entry.file_type().is_symlink()) {
        return Ok(());
    }
    if path.exists() {
        if !owned {
            return Ok(());
        }
        // `path` is assembled from compile-time agent tables plus the runtime
        // root; make sure it still resolves inside that root before any I/O.
        debug_assert!(path.starts_with(root));
        if fs::read_to_string(&path).unwrap_or_default() == content {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, content).with_context(|| format!("failed to write {}", path.display()))?;
    written.push(relative.to_owned());
    Ok(())
}

/// Cursor and Antigravity both read a `mcpServers` map: Cursor from
/// `.cursor/mcp.json`, Antigravity from `mcp_config.json`.
fn write_mcp_servers_json(path: &Path, executable: &Path, written: &mut Vec<String>) -> Result<()> {
    let mut document = read_json_object(path)?;
    let servers = servers_map(&mut document, "mcpServers", path)?;
    purge_legacy_servers(servers);
    servers.insert(
        MCP_SERVER.to_owned(),
        json!({"command": executable, "args": ["mcp"]}),
    );
    write_json(path, &document)?;
    record(path, written);
    Ok(())
}

/// OpenCode reads `opencode.json`, where local servers use `type: "local"`
/// and a `command` array instead of the `mcpServers` shape.
fn write_opencode_mcp(path: &Path, executable: &Path, written: &mut Vec<String>) -> Result<()> {
    let mut document = read_json_object(path)?;
    let servers = servers_map(&mut document, "mcp", path)?;
    purge_legacy_servers(servers);
    servers.insert(
        MCP_SERVER.to_owned(),
        json!({
            "type": "local",
            "command": [executable, "mcp"],
            "enabled": true
        }),
    );
    write_json(path, &document)?;
    record(path, written);
    Ok(())
}

/// OpenClaw nests its registry one level deeper, under `mcp.servers`, and
/// names the transport rather than inferring it from the shape.
fn write_openclaw_mcp(path: &Path, executable: &Path, written: &mut Vec<String>) -> Result<()> {
    let mut document = read_json_object(path)?;
    let mcp = servers_map(&mut document, "mcp", path)?;
    let mut nested = Value::Object(std::mem::take(mcp));
    let servers = servers_map(&mut nested, "servers", path)?;
    purge_legacy_servers(servers);
    servers.insert(
        MCP_SERVER.to_owned(),
        json!({"command": executable, "args": ["mcp"], "transport": "stdio"}),
    );
    document["mcp"] = nested;
    write_json(path, &document)?;
    record(path, written);
    Ok(())
}

/// VS Code reads `.vscode/mcp.json` under `servers`, not `mcpServers`, and
/// wants the transport named explicitly.
fn write_vscode_mcp(path: &Path, executable: &Path, written: &mut Vec<String>) -> Result<()> {
    let mut document = read_json_object(path)?;
    let servers = servers_map(&mut document, "servers", path)?;
    purge_legacy_servers(servers);
    servers.insert(
        MCP_SERVER.to_owned(),
        json!({"type": "stdio", "command": executable, "args": ["mcp"]}),
    );
    write_json(path, &document)?;
    record(path, written);
    Ok(())
}

fn read_json_object(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw).with_context(|| {
        format!(
            "invalid JSON in {}; add the `{MCP_SERVER}` entry manually",
            path.display()
        )
    })?;
    if !value.is_object() {
        bail!("{} must contain a JSON object", path.display());
    }
    Ok(value)
}

fn servers_map<'a>(
    document: &'a mut Value,
    key: &str,
    path: &Path,
) -> Result<&'a mut Map<String, Value>> {
    let object = document.as_object_mut().expect("validated JSON object");
    let servers = object.entry(key).or_insert_with(|| json!({}));
    servers
        .as_object_mut()
        .with_context(|| format!("`{key}` must be a JSON object in {}", path.display()))
}

fn purge_legacy_servers(servers: &mut Map<String, Value>) {
    for legacy in LEGACY_MCP_SERVERS {
        servers.remove(legacy);
    }
}

fn write_json(path: &Path, document: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    config::atomic_write(path, serde_json::to_string_pretty(document)?.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn record(path: &Path, written: &mut Vec<String>) {
    let label = path.display().to_string();
    if !written.contains(&label) {
        written.push(label);
    }
}

/// Best effort: the CLI errors when the server was never registered, and that
/// is the common case, so a failure here must not abort the install.
fn unregister_legacy_mcp(agent: Agent, global: bool, project: Option<&Path>) {
    // `shll` itself is included: the agent CLIs reject `mcp add` when the
    // name already exists, so a re-install must drop the stale registration
    // (possibly pointing at an old binary) before adding the current one.
    for name in [&[MCP_SERVER] as &[&str], &LEGACY_MCP_SERVERS].concat() {
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
            Agent::Hermes => {
                let mut command = Command::new("hermes");
                command.args(["mcp", "remove", name]);
                command
            }
            _ => continue,
        };
        let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

fn register_mcp_cli(
    agent: Agent,
    executable: &Path,
    global: bool,
    project: Option<&Path>,
) -> Result<()> {
    unregister_legacy_mcp(agent, global, project);
    if let Agent::Hermes = agent {
        // Hermes takes the executable and its arguments as separate flags
        // rather than after a `--` separator.
        let mut command = Command::new("hermes");
        command.args(["mcp", "add", MCP_SERVER, "--command"]);
        command.arg(executable);
        command.args(["--arg", "mcp"]);
        return run_registration(command);
    }
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
        _ => bail!(
            "{} registers through its own configuration file",
            agent.slug()
        ),
    };
    command.arg(executable).arg("mcp");
    run_registration(command)
}

fn run_registration(mut command: Command) -> Result<()> {
    let status = command
        .status()
        .context("agent CLI is not installed or not executable")?;
    if !status.success() {
        bail!("agent CLI failed to register the SafeHell MCP server");
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
    // Dedup by the hook invocation rather than the exact command, so a binary
    // that moved after an upgrade repairs the stale path instead of adding a
    // second entry that fires the same guard twice.
    if let Some(existing) = pre.iter_mut().find(|item| {
        item.pointer("/hooks/0/command")
            .and_then(Value::as_str)
            .is_some_and(|command| command.ends_with(&format!(" hook {agent}")))
    }) {
        *existing = entry;
    } else {
        pre.push(entry);
    }
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

pub fn home() -> Result<PathBuf> {
    std::env::home_dir().context("cannot determine home directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "safehell-integrations-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn fake_executable(root: &Path) -> PathBuf {
        let path = root.join("safehell-under-test");
        fs::write(&path, "#!/bin/sh\n").expect("write fake executable");
        path
    }

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

    #[test]
    fn cursor_install_registers_mcp_and_rule() {
        let root = temp_root("cursor");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        install_agent(
            Agent::Cursor,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install cursor");

        let mcp: Value = serde_json::from_str(
            &fs::read_to_string(root.join(".cursor/mcp.json")).expect("read mcp.json"),
        )
        .expect("parse mcp.json");
        assert_eq!(
            mcp.pointer("/mcpServers/shll/command"),
            Some(&json!(executable))
        );
        assert_eq!(mcp.pointer("/mcpServers/shll/args"), Some(&json!(["mcp"])));

        let rule =
            fs::read_to_string(root.join(".cursor/rules/safehell.mdc")).expect("read cursor rule");
        assert!(rule.starts_with("---\nalwaysApply: true\n---"));
    }

    #[test]
    fn cursor_install_purges_legacy_registrations() {
        let root = temp_root("cursor-legacy");
        let mcp = root.join(".cursor/mcp.json");
        fs::create_dir_all(mcp.parent().unwrap()).expect("create .cursor");
        fs::write(
            &mcp,
            r#"{"mcpServers": {"safeshell": {"command": "gone"}, "keep": {"command": "x"}}}"#,
        )
        .expect("seed legacy mcp.json");

        let executable = fake_executable(&root);
        let mut written = Vec::new();
        install_agent(
            Agent::Cursor,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install cursor");

        let mcp: Value = serde_json::from_str(&fs::read_to_string(&mcp).expect("read mcp.json"))
            .expect("parse mcp.json");
        assert!(mcp.pointer("/mcpServers/safeshell").is_none());
        assert!(mcp.pointer("/mcpServers/safehell").is_none());
        assert!(mcp.pointer("/mcpServers/keep").is_some());
        assert!(mcp.pointer("/mcpServers/shll").is_some());
    }

    #[test]
    fn opencode_install_uses_local_type() {
        let root = temp_root("opencode");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        install_agent(
            Agent::Opencode,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install opencode");

        let config: Value = serde_json::from_str(
            &fs::read_to_string(root.join("opencode.json")).expect("read opencode.json"),
        )
        .expect("parse opencode.json");
        assert_eq!(config.pointer("/mcp/shll/type"), Some(&json!("local")));
        assert_eq!(
            config.pointer("/mcp/shll/command"),
            Some(&json!([executable, "mcp"]))
        );
        assert_eq!(config.pointer("/mcp/shll/enabled"), Some(&json!(true)));
        // AGENTS.md is user-owned: seeded because it did not exist.
        assert!(root.join("AGENTS.md").is_file());
    }

    #[test]
    fn opencode_install_skips_unparseable_config() {
        let root = temp_root("opencode-jsonc");
        fs::write(root.join("opencode.json"), "{ // jsonc comment\n").expect("seed jsonc");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        let error = install_agent(
            Agent::Opencode,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect_err("jsonc must error, not be overwritten");
        assert!(error.to_string().contains("invalid JSON"));
        // The user's file is untouched.
        assert_eq!(
            fs::read_to_string(root.join("opencode.json")).expect("read jsonc"),
            "{ // jsonc comment\n"
        );
    }

    #[test]
    fn antigravity_install_writes_mcp_config_and_rule() {
        let root = temp_root("antigravity");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        install_agent(
            Agent::Antigravity,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install antigravity");

        let config: Value = serde_json::from_str(
            &fs::read_to_string(root.join(".agents/mcp_config.json"))
                .expect("read mcp_config.json"),
        )
        .expect("parse mcp_config.json");
        assert_eq!(
            config.pointer("/mcpServers/shll/command"),
            Some(&json!(executable))
        );
        let rule = fs::read_to_string(root.join(".agents/rules/safehell.md"))
            .expect("read antigravity rule");
        assert!(rule.contains("`shll` MCP tools"));
    }

    #[test]
    fn user_owned_policy_is_never_rewritten() {
        let root = temp_root("user-owned");
        fs::write(root.join("AGENTS.md"), "# My own instructions\n").expect("seed AGENTS.md");
        let mut written = Vec::new();
        seed_user_policy(&root, "AGENTS.md", &mut written).expect("seed user policy");
        assert!(written.is_empty());
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).expect("read AGENTS.md"),
            "# My own instructions\n"
        );
    }

    #[test]
    fn user_owned_policy_is_seeded_when_absent() {
        let root = temp_root("user-seed");
        let mut written = Vec::new();
        seed_user_policy(&root, "AGENTS.md", &mut written).expect("seed user policy");
        assert_eq!(written, vec!["AGENTS.md"]);
        assert!(
            fs::read_to_string(root.join("AGENTS.md"))
                .expect("read AGENTS.md")
                .contains("`shll` MCP tools")
        );
    }

    #[test]
    fn owned_policy_is_refreshed_but_stable_when_current() {
        let root = temp_root("owned");
        let mut written = Vec::new();
        seed_owned_policy(
            &root,
            ".cursor/rules/safehell.mdc",
            CURSOR_POLICY_SEED,
            &mut written,
        )
        .expect("seed owned policy");
        assert_eq!(written.len(), 1);
        let mut written = Vec::new();
        seed_owned_policy(
            &root,
            ".cursor/rules/safehell.mdc",
            CURSOR_POLICY_SEED,
            &mut written,
        )
        .expect("reseed owned policy");
        assert!(written.is_empty());
    }

    #[test]
    fn policy_seed_never_writes_through_a_symlink() {
        let root = temp_root("symlink");
        fs::write(root.join("CLAUDE.md"), "# Real file\n").expect("seed target");
        #[cfg(unix)]
        std::os::unix::fs::symlink("CLAUDE.md", root.join("AGENTS.md")).expect("create symlink");
        let mut written = Vec::new();
        seed_user_policy(&root, "AGENTS.md", &mut written).expect("seed user policy");
        assert!(written.is_empty());
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).expect("read target"),
            "# Real file\n"
        );
    }

    #[test]
    fn detection_reads_agent_markers() {
        let root = temp_root("detect");
        fs::create_dir_all(root.join(".claude")).expect("create .claude");
        fs::create_dir_all(root.join(".opencode")).expect("create .opencode");
        assert_eq!(
            detect_installed_agents(&root, false),
            vec![Agent::Claude, Agent::Opencode]
        );
        // Global markers differ: `.claude` counts, `.opencode` does not
        // (the global OpenCode marker is `.config/opencode`).
        assert_eq!(detect_installed_agents(&root, true), vec![Agent::Claude]);
    }

    #[test]
    fn bare_agents_directory_is_not_an_antigravity_marker() {
        // Codex, Cursor, and OpenCode all write `.agents/skills`, so treating
        // `.agents` as a marker would add Antigravity on every re-run.
        let root = temp_root("agents-dir");
        fs::create_dir_all(root.join(".agents/skills")).expect("create .agents/skills");
        assert!(detect_installed_agents(&root, false).is_empty());
        fs::create_dir_all(root.join(".agents/rules")).expect("create .agents/rules");
        assert_eq!(
            detect_installed_agents(&root, false),
            vec![Agent::Antigravity]
        );
    }

    #[test]
    fn openclaw_install_nests_servers_under_mcp() {
        let root = temp_root("openclaw");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        fs::write(
            root.join("openclaw.json"),
            r#"{"mcp":{"servers":{"docs":{"url":"https://example.test"},"safeshell":{}}},"plugins":{}}"#,
        )
        .expect("seed openclaw.json");
        install_agent(
            Agent::Openclaw,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install openclaw");

        let config: Value = serde_json::from_str(
            &fs::read_to_string(root.join("openclaw.json")).expect("read openclaw.json"),
        )
        .expect("parse openclaw.json");
        assert_eq!(
            config.pointer("/mcp/servers/shll/command"),
            Some(&json!(executable))
        );
        assert_eq!(
            config.pointer("/mcp/servers/shll/transport"),
            Some(&json!("stdio"))
        );
        assert_eq!(config.pointer("/mcp/servers/safeshell"), None);
        // Unrelated servers and top-level keys survive the merge.
        assert!(config.pointer("/mcp/servers/docs").is_some());
        assert!(config.pointer("/plugins").is_some());
    }

    #[test]
    fn copilot_install_uses_the_vscode_servers_key() {
        let root = temp_root("copilot");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        install_agent(
            Agent::Copilot,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install copilot");

        let config: Value = serde_json::from_str(
            &fs::read_to_string(root.join(".vscode/mcp.json")).expect("read .vscode/mcp.json"),
        )
        .expect("parse .vscode/mcp.json");
        // VS Code reads `servers`, not `mcpServers`, and needs the transport.
        assert_eq!(config.pointer("/mcpServers"), None);
        assert_eq!(config.pointer("/servers/shll/type"), Some(&json!("stdio")));
        assert_eq!(
            config.pointer("/servers/shll/command"),
            Some(&json!(executable))
        );
        assert!(root.join(".github/copilot-instructions.md").exists());
    }

    #[test]
    fn windsurf_registers_in_the_user_directory_even_for_a_project() {
        let root = temp_root("windsurf");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        install_agent(
            Agent::Windsurf,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install windsurf");

        let config: Value = serde_json::from_str(
            &fs::read_to_string(root.join(".codeium/windsurf/mcp_config.json"))
                .expect("read windsurf mcp_config.json"),
        )
        .expect("parse windsurf mcp_config.json");
        assert_eq!(
            config.pointer("/mcpServers/shll/args"),
            Some(&json!(["mcp"]))
        );
        assert!(root.join("AGENTS.md").exists());
    }

    #[test]
    fn cline_skips_registration_without_a_vscode_profile() {
        let root = temp_root("cline");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        assert!(vscode_mcp_settings(&root, Agent::Cline).is_none());
        install_agent(
            Agent::Cline,
            &root,
            &root,
            &executable,
            false,
            None,
            &mut written,
        )
        .expect("install cline");
        // The policy is still seeded, but no VS Code profile tree is invented.
        assert_eq!(written, vec!["AGENTS.md"]);
    }

    #[test]
    fn roo_registers_in_an_existing_vscode_profile() {
        let root = temp_root("roo");
        let executable = fake_executable(&root);
        let mut written = Vec::new();
        let extension = vscode_user_dir(&root)
            .expect("vscode user dir")
            .join("globalStorage/rooveterinaryinc.roo-cline");
        fs::create_dir_all(&extension).expect("create roo global storage");
        install_agent(
            Agent::Roo,
            &root,
            &root,
            &executable,
            true,
            None,
            &mut written,
        )
        .expect("install roo");

        let settings: Value = serde_json::from_str(
            &fs::read_to_string(extension.join("settings/cline_mcp_settings.json"))
                .expect("read roo mcp settings"),
        )
        .expect("parse roo mcp settings");
        assert_eq!(
            settings.pointer("/mcpServers/shll/command"),
            Some(&json!(executable))
        );
        assert!(root.join(".roo/rules/safehell.md").exists());
    }

    #[test]
    fn hook_install_repairs_stale_binary_path() {
        let root = temp_root("hook");
        let settings = root.join(".claude/settings.json");
        let old_executable = root.join("old-path/safehell");
        install_hook(&settings, &old_executable, "claude").expect("first install");

        let executable = root.join("new-path/safehell");
        install_hook(&settings, &executable, "claude").expect("second install");

        let settings: Value =
            serde_json::from_str(&fs::read_to_string(&settings).expect("read settings.json"))
                .expect("parse settings.json");
        let pre = settings
            .pointer("/hooks/PreToolUse")
            .and_then(Value::as_array)
            .expect("PreToolUse array");
        assert_eq!(pre.len(), 1, "stale entry is repaired, not duplicated");
        assert!(
            pre[0]
                .pointer("/hooks/0/command")
                .and_then(Value::as_str)
                .expect("command")
                .contains("new-path")
        );
    }
}
