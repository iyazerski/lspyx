mod diagnostics;
mod explore;
mod rename;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
    transport::stdio,
};

use self::diagnostics::DiagnosticsRequest;
use self::explore::ExploreRequest;
use self::rename::RenameRequest;
use crate::workspace::canonicalize_path;

const SERVER_INSTRUCTIONS: &str = "\
LSPYX provides agent-friendly Python LSP navigation and diagnostics. Use it \
for any Python work: `explore` to understand code, `diagnostics` to check \
changes, `rename` to preview cross-file refactors. \
Start with the smallest useful scope and set limit to keep results focused. \
Relative paths require workspace.";

#[derive(Args, Debug)]
pub(crate) struct McpArgs {
    #[command(subcommand)]
    pub(crate) command: McpSubcommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum McpSubcommand {
    /// Serve lspyx MCP over stdio.
    Serve,
}

#[derive(Clone)]
struct LspyxMcp {
    tool_router: ToolRouter<Self>,
}

impl LspyxMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[cfg(test)]
    fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LspyxMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

#[tool_router]
impl LspyxMcp {
    #[tool(
        description = "Navigate and understand Python code. Use query + workspace to find symbols, file alone to outline it, or file + line + column to inspect a symbol with its definition and usages. Use limit to keep results focused."
    )]
    fn explore(
        &self,
        Parameters(request): Parameters<ExploreRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let prepared = explore::prepare(request).map_err(invalid_params)?;
        Ok(match explore::execute(prepared) {
            Ok(output) => CallToolResult::success(vec![ContentBlock::text(output)]),
            Err(error) => tool_error(&error),
        })
    }

    #[tool(
        description = "Check Python with repository-configured Ruff and ty. Pass workspace and the smallest useful file or directory in path. Omit path only for a repo-wide check. Limit bounds returned findings, not the analysis."
    )]
    fn diagnostics(
        &self,
        Parameters(request): Parameters<DiagnosticsRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let prepared = diagnostics::prepare(request).map_err(invalid_params)?;
        diagnostics::execute(prepared)
    }

    #[tool(
        description = "Preview a semantic Python rename without changing files. Pass file + line + column + new_name, and workspace when file is relative. Returns a complete validated unified diff and structured edits."
    )]
    fn rename(
        &self,
        Parameters(request): Parameters<RenameRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let prepared = rename::prepare(request).map_err(invalid_params)?;
        Ok(match rename::execute(prepared) {
            Ok(reply) => {
                let mut result = CallToolResult::success(vec![ContentBlock::text(reply.text)]);
                result.structured_content = reply.payload;
                result
            }
            Err(error) => tool_error(&error),
        })
    }
}

/// Map a request validation failure to an MCP invalid-params protocol error.
fn invalid_params(error: anyhow::Error) -> ErrorData {
    ErrorData::invalid_params(format!("{error:#}"), None)
}

/// Report an execution failure as a tool-level error result visible to the agent.
fn tool_error(error: &anyhow::Error) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))])
}

/// Resolve a tool file argument, joining relative paths onto the workspace.
fn resolve_file(file: &Path, workspace: Option<&Path>) -> Result<PathBuf> {
    if file.is_absolute() {
        return canonicalize_path(file);
    }
    let Some(workspace) = workspace else {
        bail!("workspace is required when file is relative");
    };
    canonicalize_path(&workspace.join(file))
}

/// Run an `lspyx mcp` subcommand.
///
/// # Errors
///
/// Returns an error when the runtime cannot start or the MCP session fails.
pub(crate) fn run_mcp_command(args: McpArgs) -> Result<()> {
    match args.command {
        McpSubcommand::Serve => {
            let runtime = tokio::runtime::Runtime::new().context("failed to start MCP runtime")?;
            runtime.block_on(serve_mcp())
        }
    }
}

async fn serve_mcp() -> Result<()> {
    // rmcp answers pings and rejects other pre-initialize requests without closing the session.
    let service = LspyxMcp::new()
        .serve(stdio())
        .await
        .context("failed to initialize MCP server")?;
    service.waiting().await.context("MCP server failed")?;
    Ok(())
}

#[cfg(test)]
mod tests;
