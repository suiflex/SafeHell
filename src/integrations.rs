use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::{Agent, AgentSelection, config, executable};

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

/// Paths that show an agent is actually used here, so a bare `integrate
/// install` configures those alone instead of writing every integration into
/// every repository. Markers are the agent's own configuration directories.
const PROJECT_AGENT_MARKERS: &[(Agent, &[&str])] = &[
    (Agent::Codex, &[".codex"]),
    (Agent::Claude, &[".claude"]),
    (Agent::Cursor, &[".cursor"]),
    (Agent::Opencode, &[".opencode", "opencode.json"]),
    (Agent::Antigravity, &[".agents"]),
];

const GLOBAL_AGENT_MARKERS: &[(Agent, &[&str])] = &[
    (Agent::Codex, &[".codex"]),
    (Agent::Claude, &[".claude"]),
    (Agent::Cursor, &[".cursor"]),
    (Agent::Opencode, &[".config/opencode"]),
    (Agent::Antigravity, &[".gemini"]),
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
        AgentSelection::All => {
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
                    "no supported agent configuration found under {}; pass \
                     --agent codex,claude,cursor,opencode,antigravity to choose explicitly",
                    detection_root.display()
                );
            }
            detected
        }
    };
    let mut written = Vec::new();
    let root = agent_root(global, project.as_deref())?;
    for agent in agents {
        install_agent(
            agent,
            &root,
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
    }
    Ok(())
}

/// Report which agents leave configuration under `root`, so a bare `integrate
/// install` configures those alone.
fn detect_installed_agents(root: &Path, global: bool) -> Vec<Agent> {
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

fn home() -> Result<PathBuf> {
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
        install_agent(Agent::Cursor, &root, &executable, false, None, &mut written)
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
        install_agent(Agent::Cursor, &root, &executable, false, None, &mut written)
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
