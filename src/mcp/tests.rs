use rmcp::{handler::server::wrapper::Parameters, model::ErrorCode};

use super::LspyxMcp;
use super::diagnostics::{DiagnosticsRequest, prepare as prepare_diagnostics};
use super::explore::{
    ExploreRequest, ExploreRoute, inspect_found_no_symbol, outline_depth,
    prepare as prepare_explore, select_route, validate_query,
};
use super::rename::{RenameRequest, prepare as prepare_rename};

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
fn exposes_agent_tools() {
    let mut names = LspyxMcp::new().tool_names();
    names.sort();
    assert_eq!(names, vec!["diagnostics", "explore", "rename"]);
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
    assert_eq!(
        select_route(&request()).unwrap_err().to_string(),
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

    let error = LspyxMcp::new().explore(Parameters(request)).unwrap_err();

    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
}

#[test]
fn diagnostics_requires_workspace_without_path() {
    let error = prepare_diagnostics(DiagnosticsRequest {
        workspace: None,
        path: None,
        limit: None,
    })
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "workspace is required when path is omitted"
    );
}

#[test]
fn diagnostics_rejects_zero_limit() {
    let error = prepare_diagnostics(DiagnosticsRequest {
        workspace: Some("/tmp".into()),
        path: None,
        limit: Some(0),
    })
    .unwrap_err();

    assert_eq!(error.to_string(), "limit must be greater than 0");
}

#[test]
fn rename_rejects_empty_new_name() {
    let error = prepare_rename(RenameRequest {
        workspace: Some("/tmp".into()),
        file: "example.py".into(),
        line: 1,
        column: 1,
        new_name: "  ".to_string(),
    })
    .unwrap_err();

    assert_eq!(error.to_string(), "new_name must not be empty");
}
