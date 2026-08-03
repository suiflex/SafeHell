use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use interprocess::local_socket::tokio::prelude::*;
use serde::Serialize;

use crate::{config, ipc, remote, security, vault};

#[derive(Serialize)]
struct Audit<'a> {
    timestamp_unix: u64,
    project_id: uuid::Uuid,
    alias: &'a str,
    command_sha256: String,
    approved: bool,
    duration_ms: u128,
    exit_status: Option<i32>,
    outcome: &'a str,
}

pub async fn serve() -> Result<()> {
    let listener = ipc::listener()?;
    let busy = Arc::new(tokio::sync::Mutex::new(()));
    println!("SafeShell approval broker is running. Keep this terminal open.");
    loop {
        let stream = listener.accept().await?;
        let busy = Arc::clone(&busy);
        tokio::spawn(async move {
            let response = if let Ok(_guard) = busy.try_lock_owned() {
                match ipc::receive(&stream).await {
                    Ok(request) => {
                        handle(request)
                            .await
                            .unwrap_or_else(|error| ipc::Response::Error {
                                message: format!("{error:#}"),
                            })
                    }
                    Err(error) => ipc::Response::Error {
                        message: format!("{error:#}"),
                    },
                }
            } else {
                ipc::Response::Error {
                    message: "SafeShell broker is busy with another request".into(),
                }
            };
            if let Err(error) = ipc::respond(&stream, &response).await {
                eprintln!("broker response failed: {error:#}");
            }
        });
    }
}

async fn handle(request: ipc::Request) -> Result<ipc::Response> {
    match request {
        ipc::Request::ListServers { project } => {
            let project = exact_project(&project)?;
            Ok(ipc::Response::Servers {
                servers: project
                    .config
                    .servers
                    .into_iter()
                    .map(|(alias, server)| ipc::ServerSummary {
                        alias,
                        endpoint: format!("{}@{}:{}", server.username, server.host, server.port),
                        auth: server.auth.label().into(),
                    })
                    .collect(),
            })
        }
        ipc::Request::Execute {
            project,
            alias,
            command,
            reason,
        } => execute(&project, &alias, &command, reason.as_deref()).await,
    }
}

async fn execute(
    project_path: &Path,
    alias: &str,
    command: &str,
    reason: Option<&str>,
) -> Result<ipc::Response> {
    if command.trim().is_empty() || command.len() > 128 * 1024 {
        bail!("command must be non-empty and no larger than 128 KiB");
    }
    let project = exact_project(project_path)?;
    let server = project
        .config
        .servers
        .get(alias)
        .context("server alias not found")?;
    println!("\nApproval requested");
    println!("Project : {}", project.root.display());
    println!(
        "Server  : {alias} ({}@{}:{})",
        server.username, server.host, server.port
    );
    println!("Command : {command}");
    if let Some(reason) = reason.filter(|value| !value.trim().is_empty()) {
        println!("Reason  : {reason}");
    }
    print!("Approve once? [y/N] ");
    io::stdout().flush()?;
    let started = Instant::now();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let approved = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if !approved {
        audit(
            &project.config,
            alias,
            command,
            false,
            started.elapsed().as_millis(),
            None,
            "denied",
        )?;
        return Ok(ipc::Response::Error {
            message: "command denied by user".into(),
        });
    }
    // Fail closed before execution if the audit destination is not writable.
    audit(
        &project.config,
        alias,
        command,
        true,
        started.elapsed().as_millis(),
        None,
        "approved",
    )?;
    match remote::execute(server, &project.config.limits, command).await {
        Ok(result) => {
            if let Err(error) = audit(
                &project.config,
                alias,
                command,
                true,
                started.elapsed().as_millis(),
                result.exit_status,
                "executed",
            ) {
                eprintln!("audit finalization failed after command execution: {error:#}");
            }
            Ok(ipc::Response::Executed(result))
        }
        Err(error) => {
            if let Err(audit_error) = audit(
                &project.config,
                alias,
                command,
                true,
                started.elapsed().as_millis(),
                None,
                "failed",
            ) {
                eprintln!("audit finalization failed after command error: {audit_error:#}");
            }
            Err(error)
        }
    }
}

fn exact_project(root: &Path) -> Result<config::Project> {
    let canonical = root.canonicalize().context("project path does not exist")?;
    let project = config::discover(&canonical)?;
    if project.root.canonicalize()? != canonical {
        bail!("project request must point to the directory containing .safeshell.toml");
    }
    Ok(project)
}

fn audit(
    config: &config::ProjectConfig,
    alias: &str,
    command: &str,
    approved: bool,
    duration_ms: u128,
    exit_status: Option<i32>,
    outcome: &str,
) -> Result<()> {
    let path = vault::audit_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(
        &mut file,
        &Audit {
            timestamp_unix: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            project_id: config.project_id,
            alias,
            command_sha256: security::command_hash(command),
            approved,
            duration_ms,
            exit_status,
            outcome,
        },
    )?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}
