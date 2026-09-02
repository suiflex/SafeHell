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
    Integrate {
        #[command(subcommand)]
        command: IntegrateCommand,
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

#[derive(Subcommand)]
enum IntegrateCommand {
    Install {
        /// Install into the user's global agent configuration.
        #[arg(long)]
        global: bool,
        /// Agents to install for, comma-separated or repeated. Omit to
        /// install for every supported agent.
        #[arg(long, value_enum, value_delimiter = ',')]
        agent: Vec<Agent>,
    },
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Agent {
    Codex,
    Claude,
    Cursor,
    Opencode,
    Antigravity,
}

impl Agent {
    fn slug(self) -> &'static str {
        match self {
            Agent::Codex => "codex",
            Agent::Claude => "claude",
            Agent::Cursor => "cursor",
            Agent::Opencode => "opencode",
            Agent::Antigravity => "antigravity",
        }
    }
}

#[derive(Clone)]
enum AgentSelection {
    Explicit(Vec<Agent>),
    All,
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
        Command::Integrate {
            command: IntegrateCommand::Install { global, agent },
        } => integrations::install(agent_selection(agent), global),
        Command::Hook { agent } => integrations::hook(agent.slug()),
    }
}

/// An empty `--agent` means "every supported agent"; listing agents means
/// "exactly these". Detection narrows the set inside the integration layer.
fn agent_selection(agents: Vec<Agent>) -> AgentSelection {
    if agents.is_empty() {
        AgentSelection::All
    } else {
        AgentSelection::Explicit(agents)
    }
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
