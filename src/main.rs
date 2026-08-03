mod broker;
mod config;
mod integrations;
mod ipc;
mod mcp;
mod remote;
mod security;
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
    /// Manage servers in the current project.
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    /// Run the foreground approval broker.
    Serve,
    /// Request an approved non-interactive remote command.
    Exec {
        alias: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(required = true, last = true)]
        command: String,
    },
    /// Run the Model Context Protocol stdio adapter.
    Mcp,
    /// Install global agent integration.
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
    Install { agent: Agent },
}

#[derive(Clone, Copy, ValueEnum)]
enum Agent {
    Codex,
    Claude,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Setup => vault::setup(),
        Command::Init => config::init_project(&std::env::current_dir()?),
        Command::Server { command } => run_server_command(command),
        Command::Serve => broker::serve().await,
        Command::Exec {
            alias,
            reason,
            command,
        } => {
            let project = config::discover(&std::env::current_dir()?)?;
            let response = ipc::request(ipc::Request::Execute {
                project: project.root,
                alias,
                command,
                reason,
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
                ipc::Response::Error { message } => bail!(message),
                other => bail!("unexpected broker response: {other:?}"),
            }
        }
        Command::Mcp => mcp::run().await,
        Command::Integrate {
            command: IntegrateCommand::Install { agent },
        } => integrations::install(agent),
        Command::Hook { agent } => integrations::hook(agent),
    }
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
    std::env::current_exe().context("cannot locate safeshell executable")
}
