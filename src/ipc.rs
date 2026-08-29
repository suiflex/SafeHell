use std::path::PathBuf;

use anyhow::{Context, Result, bail};
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{
    ListenerOptions,
    tokio::{Listener, Stream, prelude::*},
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::remote::ExecutionResult;

const MAX_REQUEST_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 11 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    Execute {
        project: PathBuf,
        alias: String,
        command: String,
        reason: Option<String>,
        /// Keep only the last N lines of each stream, cut inside the broker.
        max_lines: Option<usize>,
    },
    /// Approve now, run in the background, read the output with `Poll`.
    Start {
        project: PathBuf,
        alias: String,
        command: String,
        reason: Option<String>,
    },
    Poll {
        job_id: String,
        stdout_offset: usize,
        stderr_offset: usize,
    },
    Get {
        project: PathBuf,
        alias: String,
        remote_path: String,
        local_path: PathBuf,
        reason: Option<String>,
    },
    Put {
        project: PathBuf,
        alias: String,
        local_path: PathBuf,
        remote_path: String,
        reason: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Executed(ExecutionResult),
    Started {
        job_id: String,
    },
    /// Output produced since the offsets the caller asked from.
    Progress {
        job_id: String,
        running: bool,
        stdout: String,
        stderr: String,
        stdout_offset: usize,
        stderr_offset: usize,
        exit_status: Option<i32>,
        truncated: bool,
        error: Option<String>,
    },
    Transferred {
        path: String,
        bytes: usize,
        sha256: String,
    },
    /// The command did not run. `retry_after_seconds` separates "never do this
    /// again" from "the same request may succeed later".
    Denied {
        reason: String,
        retry_after_seconds: Option<u64>,
    },
    Error {
        message: String,
    },
}

pub async fn request(request: Request) -> Result<Response> {
    let stream = Stream::connect(socket_name()?)
        .await
        .context("SafeHell broker is not running; start `safehell serve` in another terminal")?;
    write_frame(&stream, &request, MAX_REQUEST_BYTES).await?;
    read_frame(&stream, MAX_RESPONSE_BYTES).await
}

pub fn listener() -> Result<Listener> {
    ListenerOptions::new()
        .name(socket_name()?)
        .try_overwrite(true)
        .create_tokio()
        .context("another SafeHell broker may already be running")
}

pub async fn receive(stream: &Stream) -> Result<Request> {
    read_frame(stream, MAX_REQUEST_BYTES).await
}

pub async fn respond(stream: &Stream, response: &Response) -> Result<()> {
    write_frame(stream, response, MAX_RESPONSE_BYTES).await
}

#[cfg(unix)]
fn socket_name() -> Result<interprocess::local_socket::Name<'static>> {
    crate::vault::socket_path()?
        .to_fs_name::<GenericFilePath>()
        .context("cannot create broker socket path")
}

#[cfg(windows)]
fn socket_name() -> Result<interprocess::local_socket::Name<'static>> {
    let suffix = crate::vault::socket_token()?;
    format!("safehell-{suffix}")
        .to_ns_name::<GenericNamespaced>()
        .context("cannot create broker socket name")
}

async fn write_frame<T: Serialize>(stream: &Stream, value: &T, limit: usize) -> Result<()> {
    let payload = serde_json::to_vec(value)?;
    if payload.len() > limit {
        bail!("IPC frame exceeds {limit} bytes");
    }
    let mut writer = stream;
    writer.write_u32(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<T: for<'de> Deserialize<'de>>(stream: &Stream, limit: usize) -> Result<T> {
    let mut reader = stream;
    let size = reader.read_u32().await? as usize;
    if size > limit {
        bail!("IPC frame exceeds {limit} bytes");
    }
    let mut payload = vec![0; size];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).context("invalid broker IPC message")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_limit_is_bounded() {
        let request = Request::Execute {
            project: PathBuf::from("/tmp/project"),
            alias: "prod".into(),
            command: "x".repeat(MAX_REQUEST_BYTES),
            reason: None,
            max_lines: None,
        };
        assert!(serde_json::to_vec(&request).unwrap().len() > MAX_REQUEST_BYTES);
    }
}
