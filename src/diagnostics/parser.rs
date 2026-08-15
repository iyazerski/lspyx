use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{
    DiagnosticFix, DiagnosticPosition, DiagnosticRecord, DiagnosticSource, compact_message,
};

#[derive(Debug, Deserialize)]
struct RuffDiagnostic {
    code: Option<String>,
    filename: PathBuf,
    location: RuffPosition,
    end_location: RuffPosition,
    message: String,
    severity: Option<String>,
    fix: Option<RuffFix>,
}

#[derive(Debug, Deserialize)]
struct RuffPosition {
    row: usize,
    column: usize,
}

#[derive(Debug, Deserialize)]
struct RuffFix {
    applicability: Option<String>,
    message: Option<String>,
}

pub(super) fn parse_ruff_output(
    bytes: &[u8],
    workspace_root: &Path,
) -> Result<Vec<DiagnosticRecord>> {
    let diagnostics: Vec<RuffDiagnostic> =
        serde_json::from_slice(bytes).context("ruff returned invalid JSON diagnostics")?;

    Ok(diagnostics
        .into_iter()
        .map(|diagnostic| DiagnosticRecord {
            source: DiagnosticSource::Ruff,
            code: diagnostic.code.unwrap_or_else(|| "unknown".to_string()),
            severity: diagnostic.severity,
            message: compact_message(&diagnostic.message),
            file: absolute_diagnostic_path(workspace_root, &diagnostic.filename),
            start: DiagnosticPosition {
                line: diagnostic.location.row,
                column: diagnostic.location.column,
            },
            end: DiagnosticPosition {
                line: diagnostic.end_location.row,
                column: diagnostic.end_location.column,
            },
            fix: diagnostic.fix.map(|fix| DiagnosticFix {
                applicability: fix.applicability,
                message: fix.message,
            }),
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct TyDiagnostic {
    check_name: String,
    description: String,
    severity: Option<String>,
    location: TyLocation,
}

#[derive(Debug, Deserialize)]
struct TyLocation {
    path: PathBuf,
    positions: Option<TyPositions>,
    lines: Option<TyLines>,
}

#[derive(Debug, Deserialize)]
struct TyPositions {
    begin: TyPosition,
    end: TyPosition,
}

#[derive(Debug, Deserialize)]
struct TyPosition {
    line: usize,
    column: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TyLines {
    begin: usize,
    end: Option<usize>,
}

pub(super) fn parse_ty_output(
    bytes: &[u8],
    workspace_root: &Path,
) -> Result<Vec<DiagnosticRecord>> {
    let diagnostics: Vec<TyDiagnostic> =
        serde_json::from_slice(bytes).context("ty returned invalid GitLab JSON diagnostics")?;

    Ok(diagnostics
        .into_iter()
        .map(|diagnostic| {
            let (start, end) = ty_range(&diagnostic.location);
            let message = diagnostic
                .description
                .strip_prefix(&format!("{}: ", diagnostic.check_name))
                .unwrap_or(&diagnostic.description);
            DiagnosticRecord {
                source: DiagnosticSource::Ty,
                code: diagnostic.check_name,
                severity: diagnostic.severity,
                message: compact_message(message),
                file: absolute_diagnostic_path(workspace_root, &diagnostic.location.path),
                start,
                end,
                fix: None,
            }
        })
        .collect())
}

fn ty_range(location: &TyLocation) -> (DiagnosticPosition, DiagnosticPosition) {
    if let Some(positions) = &location.positions {
        return (
            DiagnosticPosition {
                line: positions.begin.line,
                column: positions.begin.column.unwrap_or(1),
            },
            DiagnosticPosition {
                line: positions.end.line,
                column: positions.end.column.unwrap_or(1),
            },
        );
    }

    let start_line = location.lines.as_ref().map_or(1, |lines| lines.begin);
    let end_line = location
        .lines
        .as_ref()
        .and_then(|lines| lines.end)
        .unwrap_or(start_line);
    (
        DiagnosticPosition {
            line: start_line,
            column: 1,
        },
        DiagnosticPosition {
            line: end_line,
            column: 1,
        },
    )
}

fn absolute_diagnostic_path(workspace_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}
