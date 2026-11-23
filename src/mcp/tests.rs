use std::fs;

use tempfile::TempDir;

use super::diagnostics::{DiagnosticsRequest, prepare as prepare_diagnostics};
use super::explore::{ExploreRequest, ExploreTarget, prepare as prepare_explore};
use super::rename::{RenameRequest, prepare as prepare_rename};
use crate::cli::SymbolKindFilter;
use crate::file_position::FilePosition;
use crate::lsp::model::OutlineDepth;

fn explore_request() -> ExploreRequest {
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
fn routes_explore_modes() {
    let dir = TempDir::new().unwrap();
    let app = dir.path().join("app.py");
    fs::write(&app, "").unwrap();
    let canonical = app.canonicalize().unwrap();

    let mut symbols = explore_request();
    symbols.workspace = Some(dir.path().to_path_buf());
    symbols.query = Some("  User ".to_string());
    symbols.kind = Some(SymbolKindFilter::Class);
    symbols.limit = Some(5);
    let prepared = prepare_explore(symbols).unwrap();
    assert_eq!(
        prepared.target,
        ExploreTarget::WorkspaceSymbols {
            query: "User".to_string(),
            kind: Some(SymbolKindFilter::Class),
        }
    );
    assert_eq!(prepared.limit, Some(5));

    for (depth, full, expected) in [
        (None, false, OutlineDepth::Limited(2)),
        (Some(3), false, OutlineDepth::Limited(3)),
        (None, true, OutlineDepth::Full),
    ] {
        let mut outline = explore_request();
        outline.query = None;
        outline.workspace = Some(dir.path().to_path_buf());
        outline.file = Some("app.py".into());
        outline.depth = depth;
        outline.full = full;
        assert_eq!(
            prepare_explore(outline).unwrap().target,
            ExploreTarget::Outline {
                file: canonical.clone(),
                depth: expected,
            }
        );
    }

    let mut position = explore_request();
    position.query = None;
    position.workspace = Some(dir.path().to_path_buf());
    position.file = Some("app.py".into());
    position.line = Some(42);
    position.column = Some(17);
    assert_eq!(
        prepare_explore(position).unwrap().target,
        ExploreTarget::Position(FilePosition {
            file: canonical,
            line: 42,
            column: 17,
        })
    );
}

#[test]
fn rejects_invalid_explore_requests() {
    type Configure = fn(&mut ExploreRequest);
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
        let mut request = explore_request();
        configure(&mut request);
        assert_eq!(prepare_explore(request).unwrap_err().to_string(), expected);
    }
}

#[test]
fn rejects_invalid_diagnostics_and_rename_requests() {
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

    let error = prepare_diagnostics(DiagnosticsRequest {
        workspace: Some("/tmp".into()),
        path: None,
        limit: Some(0),
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "limit must be greater than 0");

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
