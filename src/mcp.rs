use anyhow::{Context, Result};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::{config, ipc};

#[derive(Debug, Clone)]
struct SafeShellMcp {
    tool_router: ToolRouter<Self>,
    project: std::path::PathBuf,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ExecuteInput {
    #[schemars(description = "Server alias from .safeshell.toml")]
    alias: String,
    #[schemars(description = "Non-interactive shell command to execute remotely")]
    command: String,
    #[schemars(description = "Short explanation shown in the approval console")]
    reason: Option<String>,
    #[schemars(
        description = "Keep only the last N lines of stdout and stderr; the broker cuts before sending"
    )]
    max_lines: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct StartInput {
    #[schemars(description = "Server alias from .safeshell.toml")]
    alias: String,
    #[schemars(description = "Non-interactive shell command to execute remotely")]
    command: String,
    #[schemars(description = "Short explanation shown in the approval console")]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PollInput {
    #[schemars(description = "Job id returned by start")]
    job_id: String,
    #[schemars(description = "stdout_offset from the previous poll; omit to read from the top")]
    stdout_offset: Option<usize>,
    #[schemars(description = "stderr_offset from the previous poll; omit to read from the top")]
    stderr_offset: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetFileInput {
    #[schemars(description = "Server alias from .safeshell.toml")]
    alias: String,
    #[schemars(description = "Absolute path on the remote host")]
    remote_path: String,
    #[schemars(description = "Destination path inside the project directory")]
    local_path: String,
    #[schemars(description = "Short explanation shown in the approval console")]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PutFileInput {
    #[schemars(description = "Server alias from .safeshell.toml")]
    alias: String,
    #[schemars(description = "Source path inside the project directory")]
    local_path: String,
    #[schemars(description = "Destination path on the remote host")]
    remote_path: String,
    #[schemars(description = "Short explanation shown in the approval console")]
    reason: Option<String>,
}

impl SafeShellMcp {
    fn new(project: std::path::PathBuf) -> Self {
        Self {
            tool_router: Self::tool_router(),
            project,
        }
    }
}

#[tool_router]
impl SafeShellMcp {
    #[tool(
        description = "List configured SafeShell server aliases and their autoapprove rules. Reads the project config directly, so it works while the broker is down. Does not expose credentials."
    )]
    async fn list_servers(&self) -> String {
        match config::discover(&self.project) {
            Ok(project) if project.config.servers.is_empty() => {
                "no servers configured; run `safeshell server add`".into()
            }
            Ok(project) => project
                .config
                .servers
                .iter()
                .map(|(alias, server)| {
                    format!(
                        "{alias}\t{}@{}:{}\t{}\tallow={:?} deny={:?}",
                        server.username,
                        server.host,
                        server.port,
                        server.auth.label(),
                        server.autoapprove.allow,
                        server.autoapprove.deny,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Err(error) => format!("error: {error:#}"),
        }
    }

    #[tool(
        description = "Start a long-running command in the background after the same approval gate, and return a job_id immediately instead of blocking. Use poll to read its output progressively. This is a destructive/write-capable tool."
    )]
    async fn start(&self, Parameters(input): Parameters<StartInput>) -> String {
        response_text(
            ipc::request(ipc::Request::Start {
                project: self.project.clone(),
                alias: input.alias,
                command: input.command,
                reason: input.reason,
            })
            .await,
        )
    }

    #[tool(
        description = "Read output produced by a background job since the given offsets. Pass back the offsets from the previous poll to avoid repeating output. Partial trailing lines are withheld until the job finishes."
    )]
    async fn poll(&self, Parameters(input): Parameters<PollInput>) -> String {
        response_text(
            ipc::request(ipc::Request::Poll {
                job_id: input.job_id,
                stdout_offset: input.stdout_offset.unwrap_or(0),
                stderr_offset: input.stderr_offset.unwrap_or(0),
            })
            .await,
        )
    }

    #[tool(
        description = "Copy a remote file to a path inside the project directory, subject to the same approval gate and max_transfer_bytes. Returns size and SHA-256 only; the content is written to disk, not into this response."
    )]
    async fn get_file(&self, Parameters(input): Parameters<GetFileInput>) -> String {
        response_text(
            ipc::request(ipc::Request::Get {
                project: self.project.clone(),
                alias: input.alias,
                remote_path: input.remote_path,
                local_path: input.local_path.into(),
                reason: input.reason,
            })
            .await,
        )
    }

    #[tool(
        description = "Upload a file from inside the project directory to the remote host, subject to the same approval gate and max_transfer_bytes. This is a destructive/write-capable tool."
    )]
    async fn put_file(&self, Parameters(input): Parameters<PutFileInput>) -> String {
        response_text(
            ipc::request(ipc::Request::Put {
                project: self.project.clone(),
                alias: input.alias,
                local_path: input.local_path.into(),
                remote_path: input.remote_path,
                reason: input.reason,
            })
            .await,
        )
    }

    #[tool(
        description = "Execute a non-interactive SSH command on a configured server. Commands outside autoapprove.allow wait for a human approval that expires, so a call can come back denied. This is a destructive/write-capable tool."
    )]
    async fn execute(&self, Parameters(input): Parameters<ExecuteInput>) -> String {
        response_text(
            ipc::request(ipc::Request::Execute {
                project: self.project.clone(),
                alias: input.alias,
                command: input.command,
                reason: input.reason,
                max_lines: input.max_lines,
            })
            .await,
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SafeShellMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Use SafeShell instead of direct SSH. Every remote command requires approval in the SafeShell broker terminal.")
    }
}

pub async fn run() -> Result<()> {
    let start = std::env::var_os("CLAUDE_PROJECT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let project =
        config::discover(&start).context("MCP must be started inside a SafeShell project")?;
    SafeShellMcp::new(project.root)
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

/// Plain text, because agents read logs far better than escaped JSON blobs.
fn response_text(response: Result<ipc::Response>) -> String {
    match response {
        Ok(ipc::Response::Executed(result)) => {
            let mut text = format!(
                "status: executed\nexit_status: {}\ntruncated: {}\n",
                result
                    .exit_status
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                result.truncated
            );
            text.push_str("--- stdout ---\n");
            text.push_str(&result.stdout);
            if !result.stdout.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("--- stderr ---\n");
            text.push_str(&result.stderr);
            text
        }
        Ok(ipc::Response::Denied {
            reason,
            retry_after_seconds,
        }) => match retry_after_seconds {
            Some(seconds) => format!(
                "status: denied\nreason: {reason}\nretry_after_seconds: {seconds}\nThe command did not run. Wait at least {seconds}s before sending it again."
            ),
            None => format!(
                "status: denied\nreason: {reason}\nThe command did not run. Do not retry it; ask the operator instead."
            ),
        },
        Ok(ipc::Response::Started { job_id }) => format!(
            "status: started\njob_id: {job_id}\nThe command is running. Call poll with this job_id to read output as it arrives."
        ),
        Ok(ipc::Response::Progress {
            job_id,
            running,
            stdout,
            stderr,
            stdout_offset,
            stderr_offset,
            exit_status,
            truncated,
            error,
        }) => {
            let mut text = format!(
                "status: {}\njob_id: {job_id}\nstdout_offset: {stdout_offset}\nstderr_offset: {stderr_offset}\nexit_status: {}\ntruncated: {truncated}\n",
                if running { "running" } else { "finished" },
                exit_status
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".into()),
            );
            if let Some(error) = error {
                text.push_str(&format!("error: {error}\n"));
            }
            if running {
                text.push_str(
                    "Poll again with the offsets above to continue where this read stopped.\n",
                );
            }
            text.push_str("--- stdout ---\n");
            text.push_str(&stdout);
            if !stdout.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("--- stderr ---\n");
            text.push_str(&stderr);
            text
        }
        Ok(ipc::Response::Transferred {
            path,
            bytes,
            sha256,
        }) => format!(
            "status: transferred\npath: {path}\nbytes: {bytes}\nsha256: {sha256}\nThe file content was not loaded into this response; read the path if you need it."
        ),
        Ok(ipc::Response::Error { message }) => format!(
            "status: error\nmessage: {message}\nThis is a transport or broker failure, not a decision; retrying later may work."
        ),
        Err(error) => format!(
            "status: error\nmessage: {error:#}\nThis is a transport or broker failure, not a decision; retrying later may work."
        ),
    }
}
