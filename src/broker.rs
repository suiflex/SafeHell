use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use interprocess::local_socket::tokio::prelude::*;
use serde::Serialize;

use crate::policy::{Decision, Gate};
use crate::{config, ipc, policy, remote, security, vault};

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

pub async fn serve(auto_approve: bool) -> Result<()> {
    let listener = ipc::listener()?;
    let busy = Arc::new(tokio::sync::Mutex::new(()));
    let gate = Arc::new(std::sync::Mutex::new(Gate::default()));
    println!("SafeShell approval broker is running. Keep this terminal open.");
    if auto_approve {
        println!(
            "AUTO-APPROVE MODE: allow-listed commands run without a prompt. Anything outside `autoapprove.allow` still needs approval, and `autoapprove.deny` always wins."
        );
    }
    loop {
        let stream = listener.accept().await?;
        let busy = Arc::clone(&busy);
        let gate = Arc::clone(&gate);
        tokio::spawn(async move {
            let response = if let Ok(_guard) = busy.try_lock_owned() {
                match ipc::receive(&stream).await {
                    Ok(request) => {
                        handle(request, auto_approve, &gate)
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

async fn handle(
    request: ipc::Request,
    auto_approve: bool,
    gate: &Mutex<Gate>,
) -> Result<ipc::Response> {
    match request {
        ipc::Request::Execute {
            project,
            alias,
            command,
            reason,
            max_lines,
        } => {
            execute(
                &project,
                &alias,
                &command,
                reason.as_deref(),
                max_lines,
                auto_approve,
                gate,
            )
            .await
        }
    }
}

async fn execute(
    project_path: &Path,
    alias: &str,
    command: &str,
    reason: Option<&str>,
    max_lines: Option<usize>,
    auto_approve: bool,
    gate: &Mutex<Gate>,
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
    let limits = &project.config.limits;
    let hash = security::command_hash(command);
    let started = Instant::now();

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

    let outcome = match verdict(server, limits, &hash, command, auto_approve, gate).await? {
        Verdict::Run(outcome) => outcome,
        Verdict::Refuse {
            reason,
            retry_after_seconds,
            outcome,
        } => {
            audit(
                &project.config,
                alias,
                command,
                false,
                started.elapsed().as_millis(),
                None,
                outcome,
            )?;
            println!("Refused : {reason}");
            return Ok(ipc::Response::Denied {
                reason,
                retry_after_seconds,
            });
        }
    };

    // Fail closed before execution if the audit destination is not writable.
    audit(
        &project.config,
        alias,
        command,
        true,
        started.elapsed().as_millis(),
        None,
        outcome,
    )?;
    gate.lock()
        .expect("gate mutex poisoned")
        .record_execution(&hash, Instant::now());
    match remote::execute(server, limits, command, max_lines).await {
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

enum Verdict {
    Run(&'static str),
    Refuse {
        reason: String,
        retry_after_seconds: Option<u64>,
        outcome: &'static str,
    },
}

/// Everything that can stop a command before it reaches the remote host, in the
/// order the operator expects: deny list, duplicates, budget, then approval.
async fn verdict(
    server: &config::Server,
    limits: &config::Limits,
    hash: &str,
    command: &str,
    auto_approve: bool,
    gate: &Mutex<Gate>,
) -> Result<Verdict> {
    let refuse =
        |reason: String, retry_after_seconds: Option<u64>, outcome: &'static str| Verdict::Refuse {
            reason,
            retry_after_seconds,
            outcome,
        };
    let decision = policy::classify(&server.autoapprove, command);
    if let Decision::Blocked(rule) = decision {
        return Ok(refuse(rule, None, "blocked"));
    }
    let now = Instant::now();
    let (duplicate, throttled, ttl_approved) = {
        let mut gate = gate.lock().expect("gate mutex poisoned");
        (
            gate.duplicate_for(hash, now, limits),
            gate.throttled_for(now, limits),
            gate.approval_is_live(hash, now),
        )
    };
    if let Some(seconds) = duplicate {
        return Ok(refuse(
            format!(
                "identical command already ran less than {}s ago; reuse that result",
                limits.dedup_seconds
            ),
            Some(seconds),
            "duplicate",
        ));
    }
    if let Some(seconds) = throttled {
        return Ok(refuse(
            format!(
                "session budget of {} commands per hour is spent",
                limits.max_commands_per_hour
            ),
            Some(seconds),
            "throttled",
        ));
    }
    if decision == Decision::Allowed {
        println!("Approval : auto-approved (autoapprove.allow)");
        return Ok(Verdict::Run("auto-approved"));
    }
    if ttl_approved {
        println!(
            "Approval : covered by an approval from the last {}s",
            limits.approval_ttl_seconds
        );
        return Ok(Verdict::Run("ttl-approved"));
    }
    if auto_approve {
        return Ok(refuse(
            "broker runs unattended (--yes); only commands in autoapprove.allow execute".into(),
            None,
            "unattended",
        ));
    }
    Ok(match prompt(limits.approval_timeout_seconds).await? {
        Some(true) => {
            gate.lock().expect("gate mutex poisoned").remember_approval(
                hash,
                Instant::now(),
                limits,
            );
            Verdict::Run("approved")
        }
        Some(false) => refuse("denied by the operator".into(), None, "denied"),
        None => refuse(
            format!(
                "no operator decision within {}s",
                limits.approval_timeout_seconds
            ),
            None,
            "expired",
        ),
    })
}

/// `Some(true)` approved, `Some(false)` refused, `None` no answer in time.
///
/// ponytail: the blocking stdin read outlives the timeout, so a late keystroke
/// is swallowed by the next prompt; swap for a raw-mode reader if that bites.
async fn prompt(timeout_seconds: u64) -> Result<Option<bool>> {
    print!("Approve once? [y/N] (expires in {timeout_seconds}s) ");
    io::stdout().flush()?;
    let read = tokio::task::spawn_blocking(|| {
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).map(|_| answer)
    });
    match tokio::time::timeout(Duration::from_secs(timeout_seconds), read).await {
        Ok(joined) => Ok(Some(approves(&joined??))),
        Err(_) => {
            println!("\nApproval window expired.");
            Ok(None)
        }
    }
}

fn approves(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_yes_approves_a_command() {
        assert!(approves("y\n"));
        assert!(approves(" YES \n"));
        assert!(!approves("\n"));
        assert!(!approves("n\n"));
        assert!(!approves("yeah\n"));
    }
}
