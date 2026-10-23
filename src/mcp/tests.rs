use rmcp::{handler::server::wrapper::Parameters, model::ErrorCode};

use super::LspyxMcp;
use super::diagnostics::{DiagnosticsRequest, prepare as prepare_diagnostics};
use super::explore::{ExploreRequest, ExploreTarget, prepare as prepare_explore, select_target};
use super::rename::{RenameRequest, prepare as prepare_rename};
use crate::cli::SymbolKindFilter;

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
    request.query = Some("  User ".to_string());

    assert_eq!(
        select_target(&request).unwrap(),
        ExploreTarget::WorkspaceSymbols {
            query: "User".to_string(),
            kind: None,
        }
    );
}

#[test]
fn routes_file_only_to_outline_with_depth() {
    let mut request = request();
    request.query = None;
    request.file = Some("src/app.py".into());

    assert_eq!(
        select_target(&request).unwrap(),
        ExploreTarget::Outline {
            file: "src/app.py".into(),
            depth: Some(2),
        }
    );

    request.depth = Some(3);
    assert!(matches!(
        select_target(&request).unwrap(),
        ExploreTarget::Outline { depth: Some(3), .. }
    ));

    request.depth = None;
    request.full = true;
    assert!(matches!(
        select_target(&request).unwrap(),
        ExploreTarget::Outline { depth: None, .. }
    ));
}

#[test]
fn routes_file_position_to_position_bundle() {
    let mut request = request();
    request.query = None;
    request.file = Some("src/app.py".into());
    request.line = Some(42);
    request.column = Some(17);
    request.limit = Some(2);

    assert_eq!(
        select_target(&request).unwrap(),
        ExploreTarget::Position {
            file: "src/app.py".into(),
            line: 42,
            column: 17,
        }
    );
}

type Configure = fn(&mut ExploreRequest);

#[test]
fn rejects_invalid_explore_requests() {
    let cases: [(&str, Configure); 10] = [
        ("workspace is required when file is omitted", |_| {}),
        ("query is required when file is omitted", |request| {
            request.workspace = Some("/tmp".into());
            request.query = Some("  ".to_string());
        }),
        ("limit must be greater than 0", |request| {
            request.workspace = Some("/tmp".into());
            request.limit = Some(0);
        }),
        ("line and column require file", |request| {
            request.line = Some(42);
            request.column = Some(17);
        }),
        ("line and column must be provided together", |request| {
            request.file = Some("src/app.py".into());
            request.line = Some(42);
        }),
        ("line must be a 1-based value", |request| {
            request.file = Some("src/app.py".into());
            request.line = Some(0);
            request.column = Some(17);
        }),
        (
            "kind is only supported for workspace symbol search",
            |request| {
                request.file = Some("src/app.py".into());
                request.kind = Some(SymbolKindFilter::Class);
            },
        ),
        ("depth cannot be combined with full", |request| {
            request.file = Some("src/app.py".into());
            request.depth = Some(3);
            request.full = true;
        }),
        (
            "depth and full are only supported for file outlines",
            |request| {
                request.file = Some("src/app.py".into());
                request.line = Some(42);
                request.column = Some(17);
                request.full = true;
            },
        ),
        (
            "depth and full are only supported for file outlines",
            |request| {
                request.workspace = Some("/tmp".into());
                request.depth = Some(1);
            },
        ),
    ];

    for (expected, configure) in cases {
        let mut request = request();
        configure(&mut request);
        assert_eq!(select_target(&request).unwrap_err().to_string(), expected);
    }
}

#[test]
fn accepts_query_only_kind_and_limit() {
    let mut request = request();
    request.workspace = Some("/tmp".into());
    request.kind = Some(SymbolKindFilter::Class);
    request.limit = Some(5);

    let prepared = prepare_explore(request).unwrap();

    assert_eq!(
        prepared.target,
        ExploreTarget::WorkspaceSymbols {
            query: "User".to_string(),
            kind: Some(SymbolKindFilter::Class),
        }
    );
    assert_eq!(prepared.limit, Some(5));
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
