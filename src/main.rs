mod broker;
mod config;
mod integrations;
mod ipc;
mod mcp;
mod policy;
mod remote;
mod security;
mod update;
mod vault;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the encrypted global credential vault.
    Setup,
    /// Create a local, git-ignored project configuration.
    Init,
    /// Update SafeHell while preserving vault, project, and agent configuration.
    Update {
        /// Release tag to install; defaults to the latest published release.
        #[arg(long)]
        version: Option<String>,
    },
    /// Manage servers in the current project.
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    /// Run the foreground approval broker.
    Serve {
        /// Run unattended: allow-listed commands execute, anything that would
        /// prompt is denied instead of waiting for an operator.
        #[arg(long)]
        yes: bool,
    },
    /// Request an approved non-interactive remote command.
    Exec {
        alias: String,
        #[arg(long)]
        reason: Option<String>,
        /// Keep only the last N lines of each output stream.
        #[arg(long)]
        max_lines: Option<usize>,
        #[arg(required = true, last = true)]
        command: String,
    },
    /// Show the append-only approval and execution log.
    Audit {
        /// Number of trailing entries to print.
        #[arg(long, default_value_t = 20)]
        tail: usize,
    },
    /// Run the Model Context Protocol stdio adapter.
    Mcp,
    /// Install project-local agent integration (use --global for user-wide integration).
    Install {
        /// Install into the user's global agent configuration.
        #[arg(long)]
        global: bool,
        /// Agents to install for, comma-separated or repeated. Omit to pick
        /// interactively on a terminal, or to install for every agent whose
        /// configuration is detected.
        #[arg(long, value_enum, value_delimiter = ',')]
        agent: Vec<Agent>,
    },
    #[command(hide = true)]
    Hook { agent: Agent },
}

#[derive(Subcommand)]
enum ServerCommand {
    Add {
        alias: String,
        #[arg(long)]
        host: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        #[arg(long)]
        username: String,
        #[arg(long, value_enum, default_value_t = AuthArg::Password)]
        auth: AuthArg,
    },
    List,
    Remove {
        alias: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AuthArg {
    Password,
    SshAgent,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Agent {
    Codex,
    Claude,
    Cursor,
    Opencode,
    Hermes,
    Openclaw,
    Antigravity,
    Windsurf,
    Copilot,
    Cline,
    Roo,
}

/// Every agent, in the order the picker lists them.
const AGENTS: [Agent; 11] = [
    Agent::Codex,
    Agent::Claude,
    Agent::Cursor,
    Agent::Opencode,
    Agent::Hermes,
    Agent::Openclaw,
    Agent::Antigravity,
    Agent::Windsurf,
    Agent::Copilot,
    Agent::Cline,
    Agent::Roo,
];

impl Agent {
    fn slug(self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::Claude => "claude",
            Agent::Cursor => "cursor",
            Agent::Opencode => "opencode",
            Agent::Hermes => "hermes",
            Agent::Openclaw => "openclaw",
            Agent::Antigravity => "antigravity",
            Agent::Windsurf => "windsurf",
            Agent::Copilot => "copilot",
            Agent::Cline => "cline",
            Agent::Roo => "roo",
        }
    }

    /// What installing for this agent actually writes, so the picker states
    /// the cost of a row instead of making the reader guess from a bare name.
    fn summary(self) -> &'static str {
        match self {
            Agent::Codex => "codex mcp add, .codex/hooks.json, AGENTS.md",
            Agent::Claude => "claude mcp add, .claude/settings.json, CLAUDE.md",
            Agent::Cursor => ".cursor/mcp.json, .cursor/rules/safehell.mdc",
            Agent::Opencode => "opencode.json, AGENTS.md",
            Agent::Hermes => "hermes mcp add",
            Agent::Openclaw => "openclaw.json, AGENTS.md",
            Agent::Antigravity => "mcp_config.json, .agents/rules/safehell.md",
            Agent::Windsurf => "~/.codeium/windsurf/mcp_config.json, AGENTS.md",
            Agent::Copilot => ".vscode/mcp.json, .github/copilot-instructions.md",
            Agent::Cline => "VS Code MCP settings, AGENTS.md",
            Agent::Roo => "VS Code MCP settings, .roo/rules/safehell.md",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Setup => vault::setup(),
        Command::Init => config::init_project(&std::env::current_dir()?),
        Command::Update { version } => update::run(version.as_deref()),
        Command::Server { command } => run_server_command(command),
        Command::Serve { yes } => broker::serve(yes).await,
        Command::Exec {
            alias,
            reason,
            max_lines,
            command,
        } => {
            let project = config::discover(&std::env::current_dir()?)?;
            let response = ipc::request(ipc::Request::Execute {
                project: project.root,
                alias,
                command,
                reason,
                max_lines,
            })
            .await?;
            match response {
                ipc::Response::Executed(result) => {
                    use std::io::Write;
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(result.stdout.as_bytes())?;
                    stdout.flush()?;
                    let mut stderr = std::io::stderr().lock();
                    stderr.write_all(result.stderr.as_bytes())?;
                    stderr.flush()?;
                    std::process::exit(result.exit_status.unwrap_or(255));
                }
                // Exit 3 marks a decision, not a failure, so scripts can tell
                // "refused" apart from "the broker is down".
                ipc::Response::Denied {
                    reason,
                    retry_after_seconds,
                } => {
                    eprintln!("denied: {reason}");
                    if let Some(seconds) = retry_after_seconds {
                        eprintln!("retry after {seconds}s");
                    }
                    std::process::exit(3);
                }
                ipc::Response::Error { message } => bail!(message),
                other => bail!("unexpected broker response: {other:?}"),
            }
        }
        Command::Audit { tail } => print_audit(tail),
        Command::Mcp => mcp::run().await,
        Command::Install { global, agent } => {
            let (global, selection) = resolve_install_targets(agent, global)?;
            integrations::install(selection, global)
        }
        Command::Hook { agent } => integrations::hook(agent.slug()),
    }
}

/// Exactly three ways to decide what gets installed, in this order:
///
/// 1. An explicit `--agent` always wins and is never second-guessed, so
///    scripts and agents shelling out keep working unchanged.
/// 2. No `--agent` on a terminal opens the picker. `--global` skips it
///    because the picker asks for the scope itself.
/// 3. Otherwise install only for the agents already configured here, rather
///    than writing every integration into a repository that wanted one.
fn resolve_install_targets(
    agents: Vec<Agent>,
    global: bool,
) -> Result<(bool, integrations::AgentSelection)> {
    use std::io::IsTerminal;
    if !agents.is_empty() {
        return Ok((global, integrations::AgentSelection::Explicit(agents)));
    }
    if !global && std::io::stdout().is_terminal() {
        return run_install_picker();
    }
    Ok((global, integrations::AgentSelection::Detected))
}

const SCOPE_PROJECT: &str = "This project";
const SCOPE_GLOBAL: &str = "Global (user directory)";

fn run_install_picker() -> Result<(bool, integrations::AgentSelection)> {
    let scope = inquire::Select::new(
        "Where do you want to install?",
        vec![SCOPE_PROJECT, SCOPE_GLOBAL],
    )
    .prompt()
    .context("install cancelled")?;
    let global = scope == SCOPE_GLOBAL;

    let root = if global {
        integrations::home()?
    } else {
        std::env::current_dir()?
    };
    let detected = integrations::detect_installed_agents(&root, global);
    if detected.is_empty() {
        println!("No agent configuration found under {}.", root.display());
    } else {
        println!(
            "Detected {} — pre-selected below.",
            detected
                .iter()
                .map(|agent| agent.slug())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let width = AGENTS
        .iter()
        .map(|agent| agent.slug().len())
        .max()
        .unwrap_or_default();
    let rows: Vec<String> = AGENTS
        .iter()
        .map(|agent| format!("{:width$}  {}", agent.slug(), agent.summary()))
        .collect();
    let defaults: Vec<usize> = AGENTS
        .iter()
        .enumerate()
        .filter(|(_, agent)| detected.contains(agent))
        .map(|(index, _)| index)
        .collect();

    // Each row carries its summary, which makes a useful menu but a wrapped
    // mess once echoed back as the answer. Echo the names alone.
    let formatter = &|picked: &[inquire::list_option::ListOption<&String>]| -> String {
        picked
            .iter()
            .map(|option| option.value.split_whitespace().next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(", ")
    };
    // An empty pick must not mean "install everything": a stray Enter would
    // then write every integration at once. Ask again, then give up.
    for attempt in 0..2 {
        let picked = inquire::MultiSelect::new("Which agents?", rows.clone())
            .with_default(&defaults)
            .with_page_size(AGENTS.len())
            .with_formatter(formatter)
            .with_help_message(if attempt == 0 {
                "↑↓ move · space toggle · → all · ← none · enter confirm"
            } else {
                "nothing selected — pick at least one, or press Esc to cancel"
            })
            .prompt()
            .context("install cancelled")?;
        let chosen: Vec<Agent> = picked
            .iter()
            .filter_map(|row| {
                let slug = row.split_whitespace().next()?;
                AGENTS.iter().copied().find(|agent| agent.slug() == slug)
            })
            .collect();
        if !chosen.is_empty() {
            return Ok((global, integrations::AgentSelection::Explicit(chosen)));
        }
    }
    bail!("no agent selected; nothing was installed")
}

fn print_audit(tail: usize) -> Result<()> {
    let path = vault::audit_path()?;
    if !path.exists() {
        println!("no audit entries yet ({})", path.display());
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    println!("{} ({} entries)", path.display(), lines.len());
    for line in lines.iter().rev().take(tail).rev() {
        println!("{line}");
    }
    Ok(())
}

fn run_server_command(command: ServerCommand) -> Result<()> {
    let current = std::env::current_dir()?;
    let project = config::discover(&current)?;
    match command {
        ServerCommand::List => {
            for (alias, server) in project.config.servers {
                println!(
                    "{alias}\t{}@{}:{}\t{}",
                    server.username,
                    server.host,
                    server.port,
                    server.auth.label()
                );
            }
            Ok(())
        }
        ServerCommand::Remove { alias } => {
            let mut cfg = project.config;
            let removed = cfg
                .servers
                .remove(&alias)
                .context("server alias not found")?;
            config::save(&project.path, &cfg)?;
            if let config::Auth::Password { credential_id } = removed.auth {
                vault::remove_credential(credential_id)?;
            }
            println!("Removed {alias}");
            Ok(())
        }
        ServerCommand::Add {
            alias,
            host,
            port,
            username,
            auth,
        } => {
            config::validate_alias(&alias)?;
            if host.trim().is_empty() || username.trim().is_empty() {
                bail!("host and username must not be empty");
            }
            let mut cfg = project.config;
            if cfg.servers.contains_key(&alias) {
                bail!("server alias already exists");
            }
            let auth = match auth {
                AuthArg::SshAgent => config::Auth::SshAgent,
                AuthArg::Password => {
                    use std::io::IsTerminal;
                    if !std::io::stdin().is_terminal() {
                        bail!("password entry requires an interactive terminal");
                    }
                    let password =
                        zeroize::Zeroizing::new(rpassword::prompt_password("SSH password: ")?);
                    if password.is_empty() {
                        bail!("password must not be empty");
                    }
                    let id = vault::add_credential(&host, port, &username, password.as_str())?;
                    config::Auth::Password { credential_id: id }
                }
            };
            cfg.servers.insert(
                alias.clone(),
                config::Server {
                    host,
                    port,
                    username,
                    auth,
                    autoapprove: config::AutoApprove::default(),
                },
            );
            if let Err(error) = config::save(&project.path, &cfg) {
                if let Some(config::Server {
                    auth: config::Auth::Password { credential_id },
                    ..
                }) = cfg.servers.get(&alias)
                {
                    let _ = vault::remove_credential(*credential_id);
                }
                return Err(error);
            }
            println!("Added {alias}");
            Ok(())
        }
    }
}

fn executable() -> Result<PathBuf> {
    std::env::current_exe().context("cannot locate safehell executable")
}
