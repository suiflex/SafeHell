use std::io::{self, Write};
use std::sync::Arc;
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

pub async fn execute(
    server: &Server,
    limits: &Limits,
    command: &str,
    max_lines: Option<usize>,
) -> Result<ExecutionResult> {
    let timeout = Duration::from_secs(limits.timeout_seconds);
    let mut result = tokio::time::timeout(
        timeout,
        execute_inner(server, limits.max_output_bytes, command),
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

async fn execute_inner(
    server: &Server,
    max_output: usize,
    command: &str,
) -> Result<ExecutionResult> {
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
    let secrets: Vec<&str> = credential
        .as_ref()
        .map(|value| value.password.as_str())
        .into_iter()
        .collect();
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
