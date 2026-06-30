use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::daemon::{self, DaemonRequest};
use crate::workspace::{canonicalize_path, resolve_workspace_root};

const DEFAULT_OUTLINE_DEPTH: usize = 2;
const SERVER_INSTRUCTIONS: &str = "\
lspyx provides read-only semantic navigation for Python workspaces. Use \
lspyx_explore before grep when you need symbol search, file outlines, hover \
details, definitions, or usages. For repo-wide search, pass workspace and \
query. For relative file paths, pass workspace. For exact symbols, pass file, \
line, and column. Treat returned snippets and semantic locations as inspected.";

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

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExploreRequest {
    /// Symbol text for workspace search; required when file is omitted.
    query: Option<String>,
    /// Workspace root for query-only searches and relative file paths.
    workspace: Option<PathBuf>,
    /// Python file to outline or inspect. Relative paths require workspace.
    file: Option<PathBuf>,
    /// 1-based line for exact position inspection; use with column.
    line: Option<usize>,
    /// 1-based column for exact position inspection; use with line.
    column: Option<usize>,
    /// Maximum symbols, top-level outline entries, definitions, or usages to return.
    limit: Option<usize>,
    /// Symbol kind filter for query-only searches.
    kind: Option<SymbolKindFilter>,
    /// Outline nesting depth. Omit for the default depth; use full for the complete tree.
    depth: Option<usize>,
    /// Return the complete outline tree; cannot be combined with depth.
    #[serde(default)]
    full: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ExploreRoute {
    WorkspaceSymbols,
    Outline,
    Position,
}

#[derive(Debug)]
struct PreparedExplore {
    query: Option<String>,
    workspace_root: PathBuf,
    file: Option<PathBuf>,
    line: Option<usize>,
    column: Option<usize>,
    limit: Option<usize>,
    kind: Option<SymbolKindFilter>,
    depth: Option<usize>,
    full: bool,
    route: ExploreRoute,
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
        description = "Explore Python code in one semantic navigation call: search workspace symbols (query + workspace), outline a file (file), or inspect a position (file + line + column) with hover, definition, and usages. Use limit to bound result lists; kind for symbol searches; depth or full for outlines."
    )]
    fn lspyx_explore(
        &self,
        Parameters(request): Parameters<ExploreRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        let prepared = prepare_explore(request)
            .map_err(|error| ErrorData::invalid_params(format!("{error:#}"), None))?;

        match execute_explore(prepared) {
            Ok(output) => Ok(CallToolResult::success(vec![ContentBlock::text(output)])),
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{error:#}"
            ))])),
        }
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
    let service = LspyxMcp::new()
        .serve(stdio())
        .await
        .context("failed to initialize MCP server")?;
    service.waiting().await.context("MCP server failed")?;
    Ok(())
}

fn prepare_explore(request: ExploreRequest) -> Result<PreparedExplore> {
    let route = select_route(&request)?;
    validate_route_options(&request, &route)?;
    let query = validate_query(request.query.as_deref(), &route)?;
    let cwd = env::current_dir().context("failed to determine current directory")?;
    let file = resolve_optional_file(request.file.as_deref(), request.workspace.as_deref())?;
    let workspace_root =
        resolve_mcp_workspace(request.workspace.as_deref(), file.as_deref(), &cwd)?;

    Ok(PreparedExplore {
        query,
        workspace_root,
        file,
        line: request.line,
        column: request.column,
        limit: request.limit,
        kind: request.kind,
        depth: request.depth,
        full: request.full,
        route,
    })
}

fn execute_explore(prepared: PreparedExplore) -> Result<String> {
    match prepared.route {
        ExploreRoute::WorkspaceSymbols => daemon::run_via_daemon(
            &prepared.workspace_root,
            DaemonRequest::FindSymbol {
                query: prepared
                    .query
                    .context("query is required for workspace symbol search")?,
                kind: prepared.kind,
                limit: prepared.limit,
            },
        ),
        ExploreRoute::Outline => {
            let file = prepared
                .file
                .context("file is required for outline exploration")?;
            daemon::run_via_daemon(
                &prepared.workspace_root,
                DaemonRequest::Outline {
                    file,
                    depth: outline_depth(prepared.depth, prepared.full),
                    limit: prepared.limit,
                },
            )
        }
        ExploreRoute::Position => {
            let file = prepared
                .file
                .context("file is required for position exploration")?;
            let line = prepared
                .line
                .context("line is required for position exploration")?;
            let column = prepared
                .column
                .context("column is required for position exploration")?;
            run_position_bundle(&prepared.workspace_root, file, line, column, prepared.limit)
        }
    }
}

fn run_position_bundle(
    workspace_root: &Path,
    file: PathBuf,
    line: usize,
    column: usize,
    limit: Option<usize>,
) -> Result<String> {
    let inspect = daemon::run_via_daemon(
        workspace_root,
        DaemonRequest::Inspect {
            file: file.clone(),
            line,
            column,
        },
    )?;
    if inspect_found_no_symbol(inspect.as_str()) {
        return Ok(mcp_section("Inspect", inspect));
    }

    let definition = daemon::run_via_daemon(
        workspace_root,
        DaemonRequest::Goto {
            file: file.clone(),
            line,
            column,
            target: GotoTarget::Definition,
            limit,
        },
    )?;
    let usages = daemon::run_via_daemon(
        workspace_root,
        DaemonRequest::Usages {
            file,
            line,
            column,
            include_declaration: true,
            limit,
        },
    )?;

    Ok(format!(
        "{}\n\n{}\n\n{}",
        mcp_section("Inspect", inspect),
        mcp_section("Definition", definition),
        mcp_section("Usages", usages),
    ))
}

fn inspect_found_no_symbol(inspect: &str) -> bool {
    inspect.starts_with("no symbol found at ")
}

fn validate_query(query: Option<&str>, route: &ExploreRoute) -> Result<Option<String>> {
    if route != &ExploreRoute::WorkspaceSymbols {
        return Ok(query
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string));
    }

    let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) else {
        bail!("query is required when file is omitted");
    };
    Ok(Some(query.to_string()))
}

fn select_route(request: &ExploreRequest) -> Result<ExploreRoute> {
    match (&request.file, request.line, request.column) {
        (None, None, None) => {
            if request.workspace.is_none() {
                bail!("workspace is required when file is omitted");
            }
            Ok(ExploreRoute::WorkspaceSymbols)
        }
        (Some(_), None, None) => Ok(ExploreRoute::Outline),
        (Some(_), Some(line), Some(column)) => {
            if line == 0 {
                bail!("line must be a 1-based value");
            }
            if column == 0 {
                bail!("column must be a 1-based value");
            }
            Ok(ExploreRoute::Position)
        }
        (None, Some(_), _) | (None, _, Some(_)) => {
            bail!("line and column require file");
        }
        (Some(_), Some(_), None) | (Some(_), None, Some(_)) => {
            bail!("line and column must be provided together");
        }
    }
}

fn validate_route_options(request: &ExploreRequest, route: &ExploreRoute) -> Result<()> {
    if request.limit == Some(0) {
        bail!("limit must be greater than 0");
    }

    if request.kind.is_some() && route != &ExploreRoute::WorkspaceSymbols {
        bail!("kind is only supported for workspace symbol search");
    }

    if (request.depth.is_some() || request.full) && route != &ExploreRoute::Outline {
        bail!("depth and full are only supported for file outlines");
    }

    if request.depth.is_some() && request.full {
        bail!("depth cannot be combined with full");
    }

    Ok(())
}

fn outline_depth(depth: Option<usize>, full: bool) -> Option<usize> {
    if full {
        None
    } else {
        Some(depth.unwrap_or(DEFAULT_OUTLINE_DEPTH))
    }
}

fn resolve_optional_file(file: Option<&Path>, workspace: Option<&Path>) -> Result<Option<PathBuf>> {
    let Some(file) = file else {
        return Ok(None);
    };

    let candidate = if file.is_absolute() {
        file.to_path_buf()
    } else if let Some(workspace) = workspace {
        workspace.join(file)
    } else {
        bail!("workspace is required when file is relative");
    };

    Ok(Some(canonicalize_path(&candidate)?))
}

fn resolve_mcp_workspace(
    workspace: Option<&Path>,
    file: Option<&Path>,
    cwd: &Path,
) -> Result<PathBuf> {
    if workspace.is_none() && file.is_none() {
        bail!("workspace is required when file is omitted");
    }

    resolve_workspace_root(workspace, file, cwd)
}

fn mcp_section(title: &str, body: String) -> String {
    format!("## {title}\n\n{body}")
}

#[cfg(test)]
mod tests {
    use rmcp::{handler::server::wrapper::Parameters, model::ErrorCode};

    use super::{
        ExploreRequest, ExploreRoute, LspyxMcp, inspect_found_no_symbol, outline_depth,
        prepare_explore, select_route, validate_query,
    };

    fn request() -> ExploreRequest {
        ExploreRequest {
            query: Some("User".to_string()),
            workspace: None,
            file: None,
            line: None,
            column: None,
            limit: None,
            kind: None,
            depth: None,
            full: false,
        }
    }

    #[test]
    fn exposes_only_explore_tool() {
        assert_eq!(LspyxMcp::new().tool_names(), vec!["lspyx_explore"]);
    }

    #[test]
    fn routes_query_only_to_workspace_symbols() {
        let mut request = request();
        request.workspace = Some("/tmp/example".into());

        assert_eq!(
            select_route(&request).unwrap(),
            ExploreRoute::WorkspaceSymbols
        );
    }

    #[test]
    fn routes_file_only_to_outline() {
        let mut request = request();
        request.query = None;
        request.file = Some("src/app.py".into());

        assert_eq!(select_route(&request).unwrap(), ExploreRoute::Outline);
    }

    #[test]
    fn routes_file_position_to_position_bundle() {
        let mut request = request();
        request.query = None;
        request.file = Some("src/app.py".into());
        request.line = Some(42);
        request.column = Some(17);

        assert_eq!(select_route(&request).unwrap(), ExploreRoute::Position);
    }

    #[test]
    fn rejects_partial_position_without_file() {
        let mut request = request();
        request.line = Some(42);
        request.column = Some(17);

        assert_eq!(
            select_route(&request).unwrap_err().to_string(),
            "line and column require file"
        );
    }

    #[test]
    fn rejects_partial_position_with_file() {
        let mut request = request();
        request.file = Some("src/app.py".into());
        request.line = Some(42);

        assert_eq!(
            select_route(&request).unwrap_err().to_string(),
            "line and column must be provided together"
        );
    }

    #[test]
    fn rejects_zero_based_position_values() {
        let mut request = request();
        request.file = Some("src/app.py".into());
        request.line = Some(0);
        request.column = Some(17);

        assert_eq!(
            select_route(&request).unwrap_err().to_string(),
            "line must be a 1-based value"
        );
    }

    #[test]
    fn rejects_empty_query() {
        assert_eq!(
            validate_query(Some("  "), &ExploreRoute::WorkspaceSymbols)
                .unwrap_err()
                .to_string(),
            "query is required when file is omitted"
        );
    }

    #[test]
    fn rejects_query_only_without_workspace() {
        let request = request();

        assert_eq!(
            select_route(&request).unwrap_err().to_string(),
            "workspace is required when file is omitted"
        );
    }

    #[test]
    fn rejects_query_only_without_query() {
        let mut request = request();
        request.workspace = Some("/tmp".into());
        request.query = None;

        assert_eq!(
            prepare_explore(request).unwrap_err().to_string(),
            "query is required when file is omitted"
        );
    }

    #[test]
    fn accepts_query_only_kind_and_limit() {
        let mut request = request();
        request.workspace = Some("/tmp".into());
        request.kind = Some(crate::cli::SymbolKindFilter::Class);
        request.limit = Some(5);

        let prepared = prepare_explore(request).unwrap();

        assert_eq!(prepared.route, ExploreRoute::WorkspaceSymbols);
        assert_eq!(prepared.kind, Some(crate::cli::SymbolKindFilter::Class));
        assert_eq!(prepared.limit, Some(5));
    }

    #[test]
    fn rejects_zero_limit() {
        let mut request = request();
        request.workspace = Some("/tmp".into());
        request.limit = Some(0);

        assert_eq!(
            prepare_explore(request).unwrap_err().to_string(),
            "limit must be greater than 0"
        );
    }

    #[test]
    fn rejects_kind_for_file_routes() {
        let mut request = request();
        request.file = Some("src/app.py".into());
        request.kind = Some(crate::cli::SymbolKindFilter::Class);

        assert_eq!(
            prepare_explore(request).unwrap_err().to_string(),
            "kind is only supported for workspace symbol search"
        );
    }

    #[test]
    fn accepts_outline_depth_full_and_limit() {
        let mut request = request();
        request.query = None;
        request.file = Some("src/app.py".into());
        request.depth = Some(3);
        request.limit = Some(10);

        assert_eq!(select_route(&request).unwrap(), ExploreRoute::Outline);
        assert_eq!(outline_depth(request.depth, request.full), Some(3));

        request.depth = None;
        request.full = true;

        assert_eq!(outline_depth(request.depth, request.full), None);
    }

    #[test]
    fn rejects_outline_depth_and_full_together() {
        let mut request = request();
        request.file = Some("src/app.py".into());
        request.depth = Some(3);
        request.full = true;

        assert_eq!(
            prepare_explore(request).unwrap_err().to_string(),
            "depth cannot be combined with full"
        );
    }

    #[test]
    fn rejects_outline_options_for_position_route() {
        let mut request = request();
        request.file = Some("src/app.py".into());
        request.line = Some(42);
        request.column = Some(17);
        request.full = true;

        assert_eq!(
            prepare_explore(request).unwrap_err().to_string(),
            "depth and full are only supported for file outlines"
        );
    }

    #[test]
    fn accepts_position_limit_without_query() {
        let mut request = request();
        request.query = None;
        request.file = Some("src/app.py".into());
        request.line = Some(42);
        request.column = Some(17);
        request.limit = Some(2);

        assert_eq!(select_route(&request).unwrap(), ExploreRoute::Position);
        assert_eq!(request.limit, Some(2));
    }

    #[test]
    fn detects_no_symbol_inspect_output() {
        assert!(inspect_found_no_symbol(
            "no symbol found at src/app.py:1:1.\n\nRequested position: src/app.py:1:1"
        ));
        assert!(!inspect_found_no_symbol(
            "User is a class at src/models.py:10:7.\n\nRequested position: src/models.py:10:8"
        ));
    }

    #[test]
    fn tool_returns_invalid_params_for_invalid_file() {
        let mut request = request();
        request.file = Some("__missing__/app.py".into());

        let error = LspyxMcp::new()
            .lspyx_explore(Parameters(request))
            .unwrap_err();

        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    }
}
