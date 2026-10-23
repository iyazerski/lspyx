use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use super::adapter::PersistentAdapter;
use super::{DaemonRequest, DaemonWireResponse};
use crate::cli::GotoTarget;
use crate::lsp::model::{
    LocationOutput, OutlineOutput, RenameOutput, ResolvedPosition, SymbolAtOutput,
    WorkspaceSymbolOutput,
};
use crate::lsp::parse::{build_symbol_hierarchy, prune_outline_depth};
use crate::lsp::rename::build_rename_diff;
use crate::render::{
    render_location_output, render_outline_output, render_position_bundle, render_rename_output,
    render_symbol_at_output, render_workspace_symbol_output,
};

pub(super) enum DispatchResult {
    Respond(DaemonWireResponse),
    Shutdown(DaemonWireResponse),
}

pub(super) fn dispatch_request(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    request: DaemonRequest,
) -> Result<DispatchResult> {
    let pid_payload = || Some(json!({ "pid": std::process::id() }));
    let text = match request {
        DaemonRequest::Ping => {
            return Ok(DispatchResult::Respond(DaemonWireResponse::success(
                format!("daemon alive: {}", workspace_root.display()),
                pid_payload(),
            )));
        }
        DaemonRequest::Shutdown => {
            return Ok(DispatchResult::Shutdown(DaemonWireResponse::success(
                "daemon shutting down".to_string(),
                pid_payload(),
            )));
        }
        DaemonRequest::Goto {
            file,
            line,
            column,
            target,
            limit,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let locations = adapter.goto_locations(&position, target)?;
            render_locations(workspace_root, position, Some(target), locations, limit)
        }
        DaemonRequest::Usages {
            file,
            line,
            column,
            include_declaration,
            limit,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let locations =
                adapter.reference_locations(workspace_root, &position, include_declaration)?;
            render_locations(workspace_root, position, None, locations, limit)
        }
        DaemonRequest::FindSymbol { query, kind, limit } => {
            let (symbols, total) = adapter.search_workspace_symbols(&query, kind, limit)?;
            render_workspace_symbol_output(&WorkspaceSymbolOutput {
                workspace_root: workspace_root.to_path_buf(),
                query,
                kind,
                total,
                symbols,
            })
        }
        DaemonRequest::Inspect { file, line, column } => {
            let position = adapter.resolve_position(&file, line, column)?;
            inspect(workspace_root, adapter, position)?
        }
        DaemonRequest::Explore {
            file,
            line,
            column,
            limit,
        } => {
            // Resolve once and reuse the position for every section of the bundle.
            let position = adapter.resolve_position(&file, line, column)?;
            let inspect_text = inspect(workspace_root, adapter, position.clone())?;
            if position.symbol.is_none() {
                render_position_bundle(&[("Inspect", inspect_text)])
            } else {
                let definitions = adapter.definition_locations(&position)?;
                let usages = adapter.reference_locations(workspace_root, &position, true)?;
                let definition_text = render_locations(
                    workspace_root,
                    position.clone(),
                    Some(GotoTarget::Definition),
                    definitions,
                    limit,
                );
                let usages_text = render_locations(workspace_root, position, None, usages, limit);
                render_position_bundle(&[
                    ("Inspect", inspect_text),
                    ("Definition", definition_text),
                    ("Usages", usages_text),
                ])
            }
        }
        DaemonRequest::Outline { file, depth, limit } => {
            let symbols = build_symbol_hierarchy(adapter.document_symbols(&file)?);
            // Preserve the full tree when no depth limit is requested.
            let symbols = match depth {
                Some(depth) => prune_outline_depth(symbols, depth),
                None => symbols,
            };
            render_outline_output(
                limit,
                &OutlineOutput {
                    workspace_root: workspace_root.to_path_buf(),
                    file,
                    depth,
                    symbols,
                },
            )
        }
        DaemonRequest::Rename {
            file,
            line,
            column,
            new_name,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let old_name = position
                .symbol
                .as_ref()
                .map(|symbol| symbol.name.clone())
                .context("no symbol found at the requested rename position")?;
            // Edits are sorted by file, so adjacent grouping counts distinct files.
            let edits = adapter.rename(&position, &new_name)?;
            let payload = RenameOutput {
                workspace_root: workspace_root.to_path_buf(),
                diff: build_rename_diff(workspace_root, &old_name, &edits)?,
                position,
                old_name,
                new_name,
                total: edits.len(),
                total_files: edits
                    .chunk_by(|left, right| left.file == right.file)
                    .count(),
                edits,
            };
            return Ok(DispatchResult::Respond(DaemonWireResponse::success(
                render_rename_output(&payload),
                Some(serde_json::to_value(&payload)?),
            )));
        }
    };

    Ok(DispatchResult::Respond(DaemonWireResponse::success(
        text, None,
    )))
}

/// Render the symbol at `position` with hover details when a symbol is present.
fn inspect(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    position: ResolvedPosition,
) -> Result<String> {
    let hover = if position.symbol.is_some() {
        adapter.hover(&position)?
    } else {
        String::new()
    };
    Ok(render_symbol_at_output(&SymbolAtOutput {
        workspace_root: workspace_root.to_path_buf(),
        position,
        hover,
    }))
}

fn render_locations(
    workspace_root: &Path,
    position: ResolvedPosition,
    target: Option<GotoTarget>,
    locations: Vec<crate::lsp::model::LocationRecord>,
    limit: Option<usize>,
) -> String {
    render_location_output(
        limit,
        &LocationOutput {
            workspace_root: workspace_root.to_path_buf(),
            position,
            target,
            locations,
        },
    )
}
