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
    ListServers {
        project: PathBuf,
    },
    Execute {
        project: PathBuf,
        alias: String,
        command: String,
        reason: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Servers { servers: Vec<ServerSummary> },
    Executed(ExecutionResult),
    Error { message: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerSummary {
    pub alias: String,
    pub endpoint: String,
    pub auth: String,
}

pub async fn request(request: Request) -> Result<Response> {
    let stream = Stream::connect(socket_name()?)
        .await
        .context("SafeShell broker is not running; start `safeshell serve` in another terminal")?;
    write_frame(&stream, &request, MAX_REQUEST_BYTES).await?;
    read_frame(&stream, MAX_RESPONSE_BYTES).await
}

pub fn listener() -> Result<Listener> {
    ListenerOptions::new()
        .name(socket_name()?)
        .try_overwrite(true)
        .create_tokio()
        .context("another SafeShell broker may already be running")
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
    format!("safeshell-{suffix}")
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
        };
        assert!(serde_json::to_vec(&request).unwrap().len() > MAX_REQUEST_BYTES);
    }
}
