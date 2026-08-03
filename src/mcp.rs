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
    #[tool(description = "List configured SafeShell server aliases. Does not expose credentials.")]
    async fn list_servers(&self) -> String {
        response_text(
            ipc::request(ipc::Request::ListServers {
                project: self.project.clone(),
            })
            .await,
        )
    }

    #[tool(
        description = "Execute a non-interactive SSH command after explicit one-time user approval. This is a destructive/write-capable tool."
    )]
    async fn execute(&self, Parameters(input): Parameters<ExecuteInput>) -> String {
        response_text(
            ipc::request(ipc::Request::Execute {
                project: self.project.clone(),
                alias: input.alias,
                command: input.command,
                reason: input.reason,
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

fn response_text(response: Result<ipc::Response>) -> String {
    match response {
        Ok(value) => serde_json::to_string(&value)
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}")),
        Err(error) => format!("error: {error:#}"),
    }
}
