mod diagnostics;
mod explore;
mod paths;
mod rename;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ClientJsonRpcMessage, ClientNotification, ClientRequest, ContentBlock,
        ErrorCode, ServerCapabilities, ServerInfo, ServerJsonRpcMessage,
    },
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    tool, tool_handler, tool_router,
    transport::{Transport, async_rw::AsyncRwTransport, stdio},
};

use self::diagnostics::DiagnosticsRequest;
use self::explore::ExploreRequest;
use self::rename::RenameRequest;

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
    #[allow(
        dead_code,
        reason = "rmcp reads this field from generated tool router code"
    )]
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
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
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
        let prepared = explore::prepare(request)
            .map_err(|error| ErrorData::invalid_params(format!("{error:#}"), None))?;
        Ok(match explore::execute(prepared) {
            Ok(output) => CallToolResult::success(vec![ContentBlock::text(output)]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))]),
        })
    }

    #[tool(
        description = "Check Python with repository-configured Ruff and ty. Pass workspace and the smallest useful file or directory in path. Omit path only for a repo-wide check. Limit bounds returned findings, not the analysis."
    )]
    fn diagnostics(
        &self,
        Parameters(request): Parameters<DiagnosticsRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let prepared = diagnostics::prepare(request)
            .map_err(|error| ErrorData::invalid_params(format!("{error:#}"), None))?;
        diagnostics::execute(prepared)
    }

    #[tool(
        description = "Preview a semantic Python rename without changing files. Pass file + line + column + new_name, and workspace when file is relative. Returns a complete validated unified diff and structured edits."
    )]
    fn rename(
        &self,
        Parameters(request): Parameters<RenameRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let prepared = rename::prepare(request)
            .map_err(|error| ErrorData::invalid_params(format!("{error:#}"), None))?;
        Ok(match rename::execute(prepared) {
            Ok(response) => {
                let mut result = CallToolResult::success(vec![ContentBlock::text(
                    response.text.unwrap_or_default(),
                )]);
                result.structured_content = response.payload;
                result
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!("{error:#}"))]),
        })
    }
}

pub(crate) fn run_mcp_command(args: McpArgs) -> Result<()> {
    match args.command {
        McpSubcommand::Serve => {
            let runtime = tokio::runtime::Runtime::new().context("failed to start MCP runtime")?;
            runtime.block_on(serve_mcp())
        }
    }
}

async fn serve_mcp() -> Result<()> {
    let (stdin, stdout) = stdio();
    let transport = PreInitTransport::new(AsyncRwTransport::new_server(stdin, stdout));
    let service = LspyxMcp::new()
        .serve(transport)
        .await
        .context("failed to initialize MCP server")?;
    service.waiting().await.context("MCP server failed")?;
    Ok(())
}

struct PreInitTransport<T> {
    inner: T,
    initialized: bool,
}

impl<T> PreInitTransport<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            initialized: false,
        }
    }
}

impl<T> Transport<RoleServer> for PreInitTransport<T>
where
    T: Transport<RoleServer>,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            let message = self.inner.receive().await?;
            if self.initialized {
                return Some(message);
            }

            match message {
                ClientJsonRpcMessage::Request(request)
                    if matches!(&request.request, ClientRequest::InitializeRequest(_)) =>
                {
                    self.initialized = true;
                    return Some(ClientJsonRpcMessage::Request(request));
                }
                ClientJsonRpcMessage::Request(request)
                    if matches!(&request.request, ClientRequest::CustomRequest(_)) =>
                {
                    let error =
                        ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "Method not found", None);
                    if self
                        .inner
                        .send(ServerJsonRpcMessage::error(error, Some(request.id)))
                        .await
                        .is_err()
                    {
                        return None;
                    }
                }
                ClientJsonRpcMessage::Notification(notification)
                    if matches!(
                        notification.notification,
                        ClientNotification::CustomNotification(_)
                    ) => {}
                other => return Some(other),
            }
        }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

#[cfg(test)]
mod tests;
