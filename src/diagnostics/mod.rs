use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use self::parser::{parse_ruff_output, parse_ty_output};
use crate::lsp::model::display_path;
use crate::workspace::{locate_ruff_binary, locate_ty_binary};

pub(crate) const DEFAULT_DIAGNOSTIC_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticsOutput {
    pub(crate) complete: bool,
    pub(crate) workspace_root: PathBuf,
    pub(crate) target: PathBuf,
    pub(crate) total: usize,
    pub(crate) shown: usize,
    pub(crate) truncated: bool,
    pub(crate) tools: DiagnosticToolReports,
    pub(crate) diagnostics: Vec<DiagnosticRecord>,
}

impl DiagnosticsOutput {
    pub(crate) fn all_tools_failed(&self) -> bool {
        self.tools.ruff.status == DiagnosticToolStatus::Error
            && self.tools.ty.status == DiagnosticToolStatus::Error
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticToolReports {
    pub(crate) ruff: DiagnosticToolReport,
    pub(crate) ty: DiagnosticToolReport,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticToolReport {
    pub(crate) status: DiagnosticToolStatus,
    pub(crate) count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) binary: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiagnosticToolStatus {
    Ok,
    Error,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(crate) struct DiagnosticRecord {
    pub(crate) source: DiagnosticSource,
    pub(crate) code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) severity: Option<String>,
    pub(crate) message: String,
    pub(crate) file: PathBuf,
    pub(crate) start: DiagnosticPosition,
    pub(crate) end: DiagnosticPosition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fix: Option<DiagnosticFix>,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiagnosticSource {
    Ruff,
    Ty,
}

impl DiagnosticSource {
    fn label(self) -> &'static str {
        match self {
            Self::Ruff => "ruff",
            Self::Ty => "ty",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub(crate) struct DiagnosticPosition {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub(crate) struct DiagnosticFix {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) applicability: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

struct ToolRun {
    binary: Option<PathBuf>,
    result: Result<Vec<DiagnosticRecord>>,
}

/// Run Ruff and ty for the requested filesystem scope and normalize their diagnostics.
pub(crate) fn run_diagnostics(
    workspace_root: &Path,
    target: &Path,
    limit: usize,
) -> DiagnosticsOutput {
    let (ruff_run, ty_run) = thread::scope(|scope| {
        let ruff = scope.spawn(|| run_ruff(workspace_root, target));
        let ty = scope.spawn(|| run_ty(workspace_root, target));

        (
            ruff.join().unwrap_or_else(|_| ToolRun {
                binary: None,
                result: Err(anyhow!("ruff diagnostics worker panicked")),
            }),
            ty.join().unwrap_or_else(|_| ToolRun {
                binary: None,
                result: Err(anyhow!("ty diagnostics worker panicked")),
            }),
        )
    });

    build_output(workspace_root, target, limit, ruff_run, ty_run)
}

/// Render a compact agent-oriented summary of a diagnostics result.
pub(crate) fn render_diagnostics(output: &DiagnosticsOutput) -> String {
    let mut lines = Vec::new();
    if output.total == 0 && output.complete {
        lines.push("summary: no diagnostics (ruff and ty passed)".to_string());
    } else {
        let suffix = if output.truncated {
            format!(", showing {}", output.shown)
        } else {
            String::new()
        };
        lines.push(format!(
            "summary: {} diagnostics (ruff {}, ty {}){}",
            output.total, output.tools.ruff.count, output.tools.ty.count, suffix
        ));
    }
    lines.push(format!("complete: {}", output.complete));
    lines.push(format!("workspace: {}", output.workspace_root.display()));
    lines.push(format!(
        "target: {}",
        display_path(&output.workspace_root, &output.target)
    ));

    for (name, report) in [("ruff", &output.tools.ruff), ("ty", &output.tools.ty)] {
        if let Some(error) = report.error.as_deref() {
            lines.push(format!("{name}: failed: {error}"));
        }
    }

    if !output.diagnostics.is_empty() {
        lines.push(String::new());
    }
    for diagnostic in &output.diagnostics {
        let mut labels = vec![format!("{}/{}", diagnostic.source.label(), diagnostic.code)];
        if let Some(severity) = diagnostic.severity.as_deref() {
            labels.push(severity.to_string());
        }

        let fix = diagnostic.fix.as_ref().map_or_else(String::new, |fix| {
            let applicability = fix.applicability.as_deref().unwrap_or("available");
            format!(" ({applicability} fix)")
        });
        lines.push(format!(
            "{}:{}:{} [{}] {}{}",
            display_path(&output.workspace_root, &diagnostic.file),
            diagnostic.start.line,
            diagnostic.start.column,
            labels.join(", "),
            diagnostic.message,
            fix,
        ));
    }

    lines.join("\n")
}

fn run_ruff(workspace_root: &Path, target: &Path) -> ToolRun {
    let binary = match locate_ruff_binary(workspace_root) {
        Ok(binary) => binary,
        Err(error) => {
            return ToolRun {
                binary: None,
                result: Err(error),
            };
        }
    };

    let result = execute_ruff(&binary, workspace_root, target);
    ToolRun {
        binary: Some(binary),
        result,
    }
}

fn run_ty(workspace_root: &Path, target: &Path) -> ToolRun {
    let binary = match locate_ty_binary(workspace_root) {
        Ok(binary) => binary,
        Err(error) => {
            return ToolRun {
                binary: None,
                result: Err(error),
            };
        }
    };

    let result = execute_ty(&binary, workspace_root, target);
    ToolRun {
        binary: Some(binary),
        result,
    }
}

fn execute_ruff(
    binary: &Path,
    workspace_root: &Path,
    target: &Path,
) -> Result<Vec<DiagnosticRecord>> {
    let mut command = Command::new(binary);
    command
        .arg("check")
        .arg("--output-format")
        .arg("json")
        .arg("--color")
        .arg("never")
        .current_dir(workspace_root);

    // Keep Ruff's cache outside the diagnosed workspace.
    if let Some(cache_dir) = ruff_cache_dir(workspace_root)? {
        command.arg("--cache-dir").arg(cache_dir);
    } else {
        command.arg("--no-cache");
    }
    command.arg(target);

    let output = command
        .output()
        .with_context(|| format!("failed to run {}", binary.display()))?;
    validate_diagnostic_exit("ruff", &output, &[0, 1])?;
    parse_ruff_output(&output.stdout, workspace_root)
}

fn execute_ty(
    binary: &Path,
    workspace_root: &Path,
    target: &Path,
) -> Result<Vec<DiagnosticRecord>> {
    let output = Command::new(binary)
        .arg("check")
        .arg("--project")
        .arg(workspace_root)
        .arg("--output-format")
        .arg("gitlab")
        .arg("--no-progress")
        .arg("--color")
        .arg("never")
        .arg(target)
        .current_dir(workspace_root)
        .output()
        .with_context(|| format!("failed to run {}", binary.display()))?;
    validate_diagnostic_exit("ty", &output, &[0, 1])?;
    parse_ty_output(&output.stdout, workspace_root)
}

fn validate_diagnostic_exit(tool: &str, output: &Output, accepted: &[i32]) -> Result<()> {
    let Some(code) = output.status.code() else {
        bail!("{tool} terminated without an exit code");
    };
    if accepted.contains(&code) {
        return Ok(());
    }

    let stderr = compact_error(&String::from_utf8_lossy(&output.stderr));
    if stderr.is_empty() {
        bail!("{tool} failed with exit code {code}");
    }
    bail!("{tool} failed with exit code {code}: {stderr}")
}

fn build_output(
    workspace_root: &Path,
    target: &Path,
    limit: usize,
    ruff_run: ToolRun,
    ty_run: ToolRun,
) -> DiagnosticsOutput {
    let (ruff_report, mut ruff_diagnostics) = tool_report(ruff_run);
    let (ty_report, mut ty_diagnostics) = tool_report(ty_run);
    let complete = ruff_report.status == DiagnosticToolStatus::Ok
        && ty_report.status == DiagnosticToolStatus::Ok;

    ruff_diagnostics.append(&mut ty_diagnostics);
    ruff_diagnostics.sort_by(|left, right| {
        (
            &left.file,
            left.start.line,
            left.start.column,
            left.source,
            &left.code,
        )
            .cmp(&(
                &right.file,
                right.start.line,
                right.start.column,
                right.source,
                &right.code,
            ))
    });

    let total = ruff_diagnostics.len();
    ruff_diagnostics.truncate(limit);
    let shown = ruff_diagnostics.len();

    DiagnosticsOutput {
        complete,
        workspace_root: workspace_root.to_path_buf(),
        target: target.to_path_buf(),
        total,
        shown,
        truncated: shown < total,
        tools: DiagnosticToolReports {
            ruff: ruff_report,
            ty: ty_report,
        },
        diagnostics: ruff_diagnostics,
    }
}

fn tool_report(run: ToolRun) -> (DiagnosticToolReport, Vec<DiagnosticRecord>) {
    match run.result {
        Ok(diagnostics) => (
            DiagnosticToolReport {
                status: DiagnosticToolStatus::Ok,
                count: diagnostics.len(),
                binary: run.binary,
                error: None,
            },
            diagnostics,
        ),
        Err(error) => (
            DiagnosticToolReport {
                status: DiagnosticToolStatus::Error,
                count: 0,
                binary: run.binary,
                error: Some(format!("{error:#}")),
            },
            Vec::new(),
        ),
    }
}

fn ruff_cache_dir(workspace_root: &Path) -> Result<Option<PathBuf>> {
    let Some(home) = env::var_os("HOME") else {
        return Ok(None);
    };
    let mut hasher = DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    let cache_dir = PathBuf::from(home)
        .join(".cache")
        .join("lspyx")
        .join("ruff")
        .join(format!("{:016x}", hasher.finish()));
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("failed to create Ruff cache {}", cache_dir.display()))?;
    Ok(Some(cache_dir))
}

fn compact_message(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_error(error: &str) -> String {
    const MAX_ERROR_CHARS: usize = 2_000;
    let compact = compact_message(error);
    if compact.chars().count() <= MAX_ERROR_CHARS {
        return compact;
    }

    let mut truncated = compact.chars().take(MAX_ERROR_CHARS).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::anyhow;

    use super::{
        DiagnosticPosition, DiagnosticRecord, DiagnosticSource, ToolRun, build_output,
        parse_ruff_output, parse_ty_output, render_diagnostics,
    };

    #[test]
    fn parses_ruff_json_diagnostics() {
        let diagnostics = parse_ruff_output(
            br#"[{"code":"F401","filename":"/repo/app.py","location":{"row":1,"column":8},"end_location":{"row":1,"column":10},"message":"`os` imported but unused","severity":"error","fix":{"applicability":"safe","message":"Remove unused import"}}]"#,
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source, DiagnosticSource::Ruff);
        assert_eq!(diagnostics[0].code, "F401");
        assert_eq!(diagnostics[0].start.line, 1);
        assert_eq!(
            diagnostics[0]
                .fix
                .as_ref()
                .unwrap()
                .applicability
                .as_deref(),
            Some("safe")
        );
    }

    #[test]
    fn parses_ty_gitlab_diagnostics() {
        let diagnostics = parse_ty_output(
            br#"[{"check_name":"invalid-return-type","description":"invalid-return-type: Return type does not match","severity":"major","fingerprint":"abc","location":{"path":"src/app.py","positions":{"begin":{"line":3,"column":12},"end":{"line":3,"column":19}}}}]"#,
            Path::new("/repo"),
        )
        .unwrap();

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].source, DiagnosticSource::Ty);
        assert_eq!(diagnostics[0].file, Path::new("/repo/src/app.py"));
        assert_eq!(diagnostics[0].message, "Return type does not match");
        assert_eq!(diagnostics[0].end.column, 19);
    }

    #[test]
    fn limits_sorted_diagnostics_and_preserves_total() {
        let output = build_output(
            Path::new("/repo"),
            Path::new("/repo/src"),
            1,
            ToolRun {
                binary: Some("/bin/ruff".into()),
                result: Ok(vec![diagnostic(
                    DiagnosticSource::Ruff,
                    "/repo/src/b.py",
                    2,
                )]),
            },
            ToolRun {
                binary: Some("/bin/ty".into()),
                result: Ok(vec![diagnostic(DiagnosticSource::Ty, "/repo/src/a.py", 3)]),
            },
        );

        assert_eq!(output.total, 2);
        assert_eq!(output.shown, 1);
        assert!(output.truncated);
        assert_eq!(output.diagnostics[0].file, Path::new("/repo/src/a.py"));
    }

    #[test]
    fn reports_partial_tool_failure_with_available_diagnostics() {
        let output = build_output(
            Path::new("/repo"),
            Path::new("/repo/app.py"),
            10,
            ToolRun {
                binary: None,
                result: Err(anyhow!("ruff missing")),
            },
            ToolRun {
                binary: Some("/bin/ty".into()),
                result: Ok(vec![diagnostic(DiagnosticSource::Ty, "/repo/app.py", 4)]),
            },
        );
        let rendered = render_diagnostics(&output);

        assert!(!output.complete);
        assert!(!output.all_tools_failed());
        assert!(rendered.contains("ruff: failed: ruff missing"));
        assert!(rendered.contains("app.py:4:1 [ty/test"));
    }

    fn diagnostic(source: DiagnosticSource, file: &str, line: usize) -> DiagnosticRecord {
        DiagnosticRecord {
            source,
            code: "test".to_string(),
            severity: None,
            message: "message".to_string(),
            file: file.into(),
            start: DiagnosticPosition { line, column: 1 },
            end: DiagnosticPosition { line, column: 2 },
            fix: None,
        }
    }
}
mod parser;
