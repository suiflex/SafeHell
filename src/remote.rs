use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::known_hosts;
use russh::{ChannelMsg, Disconnect};
use serde::{Deserialize, Serialize};

use crate::config::{Auth, Limits, Server};
use crate::{security, vault};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<i32>,
    pub truncated: bool,
}

struct HostVerifier {
    host: String,
    port: u16,
    path: std::path::PathBuf,
}

impl client::Handler for HostVerifier {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match known_hosts::check_known_hosts_path(&self.host, self.port, key, &self.path) {
            Ok(true) => Ok(true),
            Err(error) => bail!("SSH host key changed: {error}"),
            Ok(false) => {
                let fingerprint = key.fingerprint(Default::default());
                eprintln!("Unknown SSH host key for {}:{}", self.host, self.port);
                eprintln!("Fingerprint: {fingerprint}");
                eprint!("Trust and save this host key? [y/N] ");
                io::stderr().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    return Ok(false);
                }
                known_hosts::learn_known_hosts_path(&self.host, self.port, key, &self.path)?;
                Ok(true)
            }
        }
    }
}

/// Output of a command that may still be running, shared with pollers.
#[derive(Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<i32>,
    pub truncated: bool,
    pub finished: bool,
    pub error: Option<String>,
}

/// Run a command, publishing redacted output into `sink` as it arrives so a
/// caller can read the beginning before the command ends.
pub async fn execute_streamed(
    server: &Server,
    limits: &Limits,
    command: &str,
    sink: Arc<Mutex<Output>>,
) {
    let outcome = tokio::time::timeout(
        Duration::from_secs(limits.timeout_seconds),
        execute_inner(server, limits.max_output_bytes, command, Some(&sink)),
    )
    .await
    .map_err(|_| anyhow::anyhow!("remote command timed out"))
    .and_then(|inner| inner);
    let mut guard = sink.lock().expect("job sink poisoned");
    match outcome {
        Ok(result) => {
            guard.stdout = result.stdout;
            guard.stderr = result.stderr;
            guard.exit_status = result.exit_status;
            guard.truncated = result.truncated;
        }
        Err(error) => guard.error = Some(format!("{error:#}")),
    }
    guard.finished = true;
}

pub async fn execute(
    server: &Server,
    limits: &Limits,
    command: &str,
    max_lines: Option<usize>,
) -> Result<ExecutionResult> {
    let timeout = Duration::from_secs(limits.timeout_seconds);
    let mut result = tokio::time::timeout(
        timeout,
        execute_inner(server, limits.max_output_bytes, command, None),
    )
    .await
    .context("remote command timed out")??;
    if let Some(limit) = max_lines {
        let (stdout, cut_out) = tail(&result.stdout, limit);
        let (stderr, cut_err) = tail(&result.stderr, limit);
        result.stdout = stdout;
        result.stderr = stderr;
        result.truncated |= cut_out || cut_err;
    }
    Ok(result)
}

/// Shell-quote a path so an operator sees exactly what the remote shell runs.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The command the operator approves for a download; `limit + 1` bytes are
/// requested so an oversized file is detected instead of silently cut.
pub fn download_command(path: &str, limit: usize) -> String {
    format!("head -c {} -- {} | base64", limit + 1, quote(path))
}

pub fn upload_command(path: &str) -> String {
    format!("base64 -d > {}", quote(path))
}

/// Fetch a remote file as bytes. Output is not redacted: base64 hides nothing
/// from a scrubber, so a transfer is protected by the approval gate alone.
pub async fn download(server: &Server, limits: &Limits, path: &str) -> Result<Vec<u8>> {
    let limit = limits.max_transfer_bytes;
    // base64 grows by 4/3 and wraps lines, so allow generous headroom.
    let budget = limit.saturating_mul(2).saturating_add(1024);
    let result = tokio::time::timeout(
        Duration::from_secs(limits.timeout_seconds),
        raw_exec(server, budget, &download_command(path, limit)),
    )
    .await
    .context("remote download timed out")??;
    if result.exit_status.unwrap_or(1) != 0 {
        bail!(
            "remote read failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    let encoded: String = String::from_utf8_lossy(&result.stdout)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .context("remote output was not valid base64")?;
    if bytes.len() > limit {
        bail!("file is larger than max_transfer_bytes ({limit})");
    }
    Ok(bytes)
}

pub async fn upload(server: &Server, limits: &Limits, path: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() > limits.max_transfer_bytes {
        bail!(
            "file is larger than max_transfer_bytes ({})",
            limits.max_transfer_bytes
        );
    }
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    tokio::time::timeout(
        Duration::from_secs(limits.timeout_seconds),
        upload_inner(server, path, encoded),
    )
    .await
    .context("remote upload timed out")?
}

async fn upload_inner(server: &Server, path: &str, encoded: String) -> Result<()> {
    let (session, _credential) = connect(server).await?;
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, upload_command(path)).await?;
    channel.data(encoded.as_bytes()).await?;
    channel.eof().await?;
    let mut exit_status = None;
    let mut stderr = Vec::new();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus {
                exit_status: status,
            } => exit_status = i32::try_from(status).ok(),
            _ => {}
        }
    }
    session
        .disconnect(Disconnect::ByApplication, "", "English")
        .await?;
    if exit_status.unwrap_or(1) != 0 {
        bail!(
            "remote write failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    Ok(())
}

struct RawResult {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_status: Option<i32>,
}

async fn raw_exec(server: &Server, max_output: usize, command: &str) -> Result<RawResult> {
    let (session, _credential) = connect(server).await?;
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;
    let mut truncated = false;
    while let Some(message) = channel.wait().await {
        let remaining = max_output.saturating_sub(stdout.len() + stderr.len());
        match message {
            ChannelMsg::Data { data } => {
                append_bounded(&mut stdout, &data, remaining, &mut truncated)
            }
            ChannelMsg::ExtendedData { data, .. } => {
                append_bounded(&mut stderr, &data, remaining, &mut truncated)
            }
            ChannelMsg::ExitStatus {
                exit_status: status,
            } => exit_status = i32::try_from(status).ok(),
            _ => {}
        }
    }
    session
        .disconnect(Disconnect::ByApplication, "", "English")
        .await?;
    if truncated {
        bail!("transfer exceeded the output budget");
    }
    Ok(RawResult {
        stdout,
        stderr,
        exit_status,
    })
}

/// Keep the last `limit` lines; the broker cuts before the response is sent.
fn tail(text: &str, limit: usize) -> (String, bool) {
    if limit == 0 {
        return (String::new(), !text.is_empty());
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= limit {
        return (text.to_string(), false);
    }
    (lines[lines.len() - limit..].join("\n"), true)
}

async fn connect(
    server: &Server,
) -> Result<(client::Handle<HostVerifier>, Option<vault::Credential>)> {
    let handler = HostVerifier {
        host: server.host.clone(),
        port: server.port,
        path: vault::known_hosts_path()?,
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let mut session = client::connect(config, (&*server.host, server.port), handler)
        .await
        .with_context(|| format!("cannot connect to {}:{}", server.host, server.port))?;

    let credential = match &server.auth {
        Auth::Password { credential_id } => {
            let credential =
                vault::credential(*credential_id, &server.host, server.port, &server.username)?;
            let auth = session
                .authenticate_password(&server.username, &credential.password)
                .await?;
            if !auth.success() {
                bail!("SSH password authentication failed");
            }
            Some(credential)
        }
        Auth::SshAgent => {
            authenticate_agent(&mut session, &server.username).await?;
            None
        }
    };
    Ok((session, credential))
}

async fn execute_inner(
    server: &Server,
    max_output: usize,
    command: &str,
    sink: Option<&Arc<Mutex<Output>>>,
) -> Result<ExecutionResult> {
    let (session, credential) = connect(server).await?;
    let secrets: Vec<&str> = credential
        .as_ref()
        .map(|value| value.password.as_str())
        .into_iter()
        .collect();
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, command).await?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_status = None;
    let mut truncated = false;
    while let Some(message) = channel.wait().await {
        let remaining = max_output.saturating_sub(stdout.len() + stderr.len());
        match message {
            ChannelMsg::Data { data } => {
                append_bounded(&mut stdout, &data, remaining, &mut truncated)
            }
            ChannelMsg::ExtendedData { data, .. } => {
                append_bounded(&mut stderr, &data, remaining, &mut truncated)
            }
            ChannelMsg::ExitStatus {
                exit_status: status,
            } => exit_status = i32::try_from(status).ok(),
            _ => {}
        }
        if let Some(sink) = sink {
            let mut guard = sink.lock().expect("job sink poisoned");
            guard.stdout = security::redact(&stdout, &secrets);
            guard.stderr = security::redact(&stderr, &secrets);
            guard.truncated = truncated;
        }
    }
    session
        .disconnect(Disconnect::ByApplication, "", "English")
        .await?;
    Ok(ExecutionResult {
        stdout: security::redact(&stdout, &secrets),
        stderr: security::redact(&stderr, &secrets),
        exit_status,
        truncated,
    })
}

#[cfg(unix)]
async fn authenticate_agent(
    session: &mut client::Handle<HostVerifier>,
    username: &str,
) -> Result<()> {
    let mut agent = russh::keys::agent::client::AgentClient::connect_env()
        .await
        .context("cannot connect to SSH_AUTH_SOCK")?;
    let identities = agent.request_identities().await?;
    if identities.is_empty() {
        bail!("SSH agent contains no identities");
    }
    for identity in identities {
        let key = identity.public_key().into_owned();
        let hash = session.best_supported_rsa_hash().await?.flatten();
        if session
            .authenticate_publickey_with(username, key, hash, &mut agent)
            .await?
            .success()
        {
            return Ok(());
        }
    }
    bail!("SSH agent authentication failed")
}

#[cfg(windows)]
async fn authenticate_agent(
    session: &mut client::Handle<HostVerifier>,
    username: &str,
) -> Result<()> {
    let mut agent =
        russh::keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .context("cannot connect to Windows OpenSSH agent")?;
    let identities = agent.request_identities().await?;
    for identity in identities {
        let key = identity.public_key().into_owned();
        let hash = session.best_supported_rsa_hash().await?.flatten();
        if session
            .authenticate_publickey_with(username, key, hash, &mut agent)
            .await?
            .success()
        {
            return Ok(());
        }
    }
    bail!("SSH agent authentication failed")
}

fn append_bounded(target: &mut Vec<u8>, bytes: &[u8], remaining: usize, truncated: &mut bool) {
    let take = bytes.len().min(remaining);
    target.extend_from_slice(&bytes[..take]);
    *truncated |= take != bytes.len();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_commands_quote_their_paths() {
        assert_eq!(
            download_command("/var/log/app's.log", 10),
            "head -c 11 -- '/var/log/app'\\''s.log' | base64"
        );
        assert_eq!(upload_command("/tmp/x y"), "base64 -d > '/tmp/x y'");
    }

    #[test]
    fn tail_keeps_the_last_lines_only() {
        assert_eq!(tail("a\nb\nc", 2), ("b\nc".into(), true));
        assert_eq!(tail("a\nb", 5), ("a\nb".into(), false));
        assert_eq!(tail("a\nb", 0), (String::new(), true));
    }

    #[test]
    fn output_is_bounded() {
        let mut output = vec![1, 2];
        let mut truncated = false;
        append_bounded(&mut output, &[3, 4, 5], 2, &mut truncated);
        assert_eq!(output, [1, 2, 3, 4]);
        assert!(truncated);
    }
}
