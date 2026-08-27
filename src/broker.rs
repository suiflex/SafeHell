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

/// Broker-wide state. The terminal lock exists because the approval prompt is a
/// single shared console; everything else may run concurrently so that polling a
/// background job is never stuck behind a long command.
pub struct State {
    auto_approve: bool,
    gate: Mutex<Gate>,
    terminal: tokio::sync::Mutex<()>,
    jobs: Mutex<Vec<(String, Arc<Mutex<remote::Output>>)>>,
}

/// ponytail: keep a handful of finished jobs for late pollers, drop the oldest.
const MAX_JOBS: usize = 16;

pub async fn serve(auto_approve: bool) -> Result<()> {
    let listener = ipc::listener()?;
    let state = Arc::new(State {
        auto_approve,
        gate: Mutex::new(Gate::default()),
        terminal: tokio::sync::Mutex::new(()),
        jobs: Mutex::new(Vec::new()),
    });
    println!("SafeShell approval broker is running. Keep this terminal open.");
    if auto_approve {
        println!(
            "AUTO-APPROVE MODE: allow-listed commands run without a prompt. Anything outside `autoapprove.allow` still needs approval, and `autoapprove.deny` always wins."
        );
    }
    loop {
        let stream = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let response = match ipc::receive(&stream).await {
                Ok(request) => {
                    handle(request, &state)
                        .await
                        .unwrap_or_else(|error| ipc::Response::Error {
                            message: format!("{error:#}"),
                        })
                }
                Err(error) => ipc::Response::Error {
                    message: format!("{error:#}"),
                },
            };
            if let Err(error) = ipc::respond(&stream, &response).await {
                eprintln!("broker response failed: {error:#}");
            }
        });
    }
}

async fn handle(request: ipc::Request, state: &Arc<State>) -> Result<ipc::Response> {
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
                state,
            )
            .await
        }
        ipc::Request::Start {
            project,
            alias,
            command,
            reason,
        } => start(&project, &alias, &command, reason.as_deref(), state).await,
        ipc::Request::Poll {
            job_id,
            stdout_offset,
            stderr_offset,
        } => poll(&job_id, stdout_offset, stderr_offset, state),
        ipc::Request::Get {
            project,
            alias,
            remote_path,
            local_path,
            reason,
        } => {
            transfer(
                &project,
                &alias,
                Transfer::Get {
                    remote_path,
                    local_path,
                },
                reason.as_deref(),
                state,
            )
            .await
        }
        ipc::Request::Put {
            project,
            alias,
            local_path,
            remote_path,
            reason,
        } => {
            transfer(
                &project,
                &alias,
                Transfer::Put {
                    local_path,
                    remote_path,
                },
                reason.as_deref(),
                state,
            )
            .await
        }
    }
}

/// Approval, policy, and audit for one command. `Err` on the inner result is the
/// finished refusal response, so every caller refuses the same way.
async fn gated(
    project: &config::Project,
    alias: &str,
    command: &str,
    reason: Option<&str>,
    state: &State,
    started: Instant,
) -> Result<Result<(), ipc::Response>> {
    if command.trim().is_empty() || command.len() > 128 * 1024 {
        bail!("command must be non-empty and no larger than 128 KiB");
    }
    let server = project
        .config
        .servers
        .get(alias)
        .context("server alias not found")?;
    let limits = &project.config.limits;
    let hash = security::command_hash(command);

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

    let outcome = match verdict(server, limits, &hash, command, state).await? {
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
            return Ok(Err(ipc::Response::Denied {
                reason,
                retry_after_seconds,
            }));
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
    state
        .gate
        .lock()
        .expect("gate mutex poisoned")
        .record_execution(&hash, Instant::now());
    Ok(Ok(()))
}

async fn execute(
    project_path: &Path,
    alias: &str,
    command: &str,
    reason: Option<&str>,
    max_lines: Option<usize>,
    state: &State,
) -> Result<ipc::Response> {
    let project = exact_project(project_path)?;
    let started = Instant::now();
    if let Err(refusal) = gated(&project, alias, command, reason, state, started).await? {
        return Ok(refusal);
    }
    let server = &project.config.servers[alias];
    let limits = &project.config.limits;
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

async fn start(
    project_path: &Path,
    alias: &str,
    command: &str,
    reason: Option<&str>,
    state: &Arc<State>,
) -> Result<ipc::Response> {
    let project = exact_project(project_path)?;
    let started = Instant::now();
    if let Err(refusal) = gated(&project, alias, command, reason, state, started).await? {
        return Ok(refusal);
    }
    let job_id = uuid::Uuid::new_v4().to_string();
    let sink = Arc::new(Mutex::new(remote::Output::default()));
    {
        let mut jobs = state.jobs.lock().expect("job registry poisoned");
        jobs.push((job_id.clone(), Arc::clone(&sink)));
        while jobs.len() > MAX_JOBS {
            jobs.remove(0);
        }
    }
    let server = project.config.servers[alias].clone();
    let limits = project.config.limits.clone();
    let command = command.to_string();
    let config = project.config.clone();
    let alias = alias.to_string();
    tokio::spawn(async move {
        remote::execute_streamed(&server, &limits, &command, Arc::clone(&sink)).await;
        let (exit_status, failed) = {
            let output = sink.lock().expect("job sink poisoned");
            (output.exit_status, output.error.is_some())
        };
        if let Err(error) = audit(
            &config,
            &alias,
            &command,
            true,
            started.elapsed().as_millis(),
            exit_status,
            if failed { "failed" } else { "executed" },
        ) {
            eprintln!("audit finalization failed for background job: {error:#}");
        }
    });
    println!("Started : background job {job_id}");
    Ok(ipc::Response::Started { job_id })
}

fn poll(
    job_id: &str,
    stdout_offset: usize,
    stderr_offset: usize,
    state: &State,
) -> Result<ipc::Response> {
    let sink = {
        let jobs = state.jobs.lock().expect("job registry poisoned");
        jobs.iter()
            .find(|(id, _)| id == job_id)
            .map(|(_, sink)| Arc::clone(sink))
            .context("unknown job id; it may have been evicted after newer jobs")?
    };
    let output = sink.lock().expect("job sink poisoned");
    let finished = output.finished;
    let stdout = security::releasable(&output.stdout, finished);
    let stderr = security::releasable(&output.stderr, finished);
    Ok(ipc::Response::Progress {
        job_id: job_id.to_string(),
        running: !finished,
        stdout: stdout.get(stdout_offset..).unwrap_or_default().to_string(),
        stderr: stderr.get(stderr_offset..).unwrap_or_default().to_string(),
        stdout_offset: stdout.len(),
        stderr_offset: stderr.len(),
        exit_status: output.exit_status,
        truncated: output.truncated,
        error: output.error.clone(),
    })
}

enum Transfer {
    Get {
        remote_path: String,
        local_path: std::path::PathBuf,
    },
    Put {
        local_path: std::path::PathBuf,
        remote_path: String,
    },
}

/// Move the bytes once the request is approved. `local` is already confined to
/// the project by `local_in_project`.
async fn move_bytes(
    server: &config::Server,
    limits: &config::Limits,
    transfer: Transfer,
    local: &Path,
) -> Result<(String, Vec<u8>)> {
    match transfer {
        Transfer::Get { remote_path, .. } => {
            let bytes = remote::download(server, limits, &remote_path).await?;
            config::atomic_write(local, &bytes)?;
            Ok((local.display().to_string(), bytes))
        }
        Transfer::Put { remote_path, .. } => {
            let bytes =
                std::fs::read(local).with_context(|| format!("cannot read {}", local.display()))?;
            remote::upload(server, limits, &remote_path, &bytes).await?;
            Ok((remote_path, bytes))
        }
    }
}

async fn transfer(
    project_path: &Path,
    alias: &str,
    transfer: Transfer,
    reason: Option<&str>,
    state: &State,
) -> Result<ipc::Response> {
    let project = exact_project(project_path)?;
    let limits = &project.config.limits;
    let command = match &transfer {
        Transfer::Get { remote_path, .. } => {
            remote::download_command(remote_path, limits.max_transfer_bytes)
        }
        Transfer::Put { remote_path, .. } => remote::upload_command(remote_path),
    };
    // Refuse an out-of-project path before spending an operator's approval on it.
    let local = match &transfer {
        Transfer::Get { local_path, .. } | Transfer::Put { local_path, .. } => {
            match local_in_project(&project, local_path) {
                Ok(local) => local,
                Err(error) => {
                    return Ok(ipc::Response::Denied {
                        reason: format!("{error:#}"),
                        retry_after_seconds: None,
                    });
                }
            }
        }
    };
    let started = Instant::now();
    if let Err(refusal) = gated(&project, alias, &command, reason, state, started).await? {
        return Ok(refusal);
    }
    let server = &project.config.servers[alias];
    let (path, bytes) = match move_bytes(server, limits, transfer, &local).await {
        Ok(value) => value,
        Err(error) => {
            // A capped or missing file is a decision about this request, not a
            // broker failure, so the caller is told not to repeat it blindly.
            audit(
                &project.config,
                alias,
                &command,
                false,
                started.elapsed().as_millis(),
                None,
                "transfer-failed",
            )?;
            return Ok(ipc::Response::Denied {
                reason: format!("{error:#}"),
                retry_after_seconds: None,
            });
        }
    };
    if let Err(error) = audit(
        &project.config,
        alias,
        &command,
        true,
        started.elapsed().as_millis(),
        Some(0),
        "transferred",
    ) {
        eprintln!("audit finalization failed after transfer: {error:#}");
    }
    Ok(ipc::Response::Transferred {
        path,
        bytes: bytes.len(),
        sha256: security::bytes_hash(&bytes),
    })
}

/// Local transfer endpoints stay inside the project, so an agent cannot use the
/// broker to read or overwrite files elsewhere on the workstation.
fn local_in_project(project: &config::Project, path: &Path) -> Result<std::path::PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.root.join(path)
    };
    let root = project.root.canonicalize()?;
    let parent = candidate
        .parent()
        .context("local path has no parent directory")?
        .canonicalize()
        .with_context(|| format!("no such directory: {}", candidate.display()))?;
    if !parent.starts_with(&root) {
        bail!("local path must stay inside the project directory");
    }
    Ok(parent.join(
        candidate
            .file_name()
            .context("local path has no file name")?,
    ))
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
    state: &State,
) -> Result<Verdict> {
    let gate = &state.gate;
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
    if state.auto_approve {
        return Ok(refuse(
            "broker runs unattended (--yes); only commands in autoapprove.allow execute".into(),
            None,
            "unattended",
        ));
    }
    // One prompt at a time: the console is shared by every in-flight request.
    let _console = state.terminal.lock().await;
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

    #[test]
    fn transfer_paths_cannot_escape_the_project() {
        let root = std::env::temp_dir().join(format!("safeshell-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let project = config::Project {
            path: root.join(config::CONFIG_NAME),
            config: config::ProjectConfig {
                version: 1,
                project_id: uuid::Uuid::nil(),
                limits: config::Limits::default(),
                servers: std::collections::BTreeMap::new(),
            },
            root: root.clone(),
        };
        assert!(local_in_project(&project, Path::new("logs/app.log")).is_ok());
        assert!(local_in_project(&project, Path::new("../escape.log")).is_err());
        assert!(local_in_project(&project, Path::new("/etc/passwd")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
