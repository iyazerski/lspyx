use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use super::adapter::PersistentAdapter;
use super::{DaemonRequest, DaemonWireResponse};
use crate::cli::GotoTarget;
use crate::lsp::model::{
    LocationOutput, OutlineOutput, RenameOutput, SymbolAtOutput, WorkspaceSymbolOutput,
};
use crate::lsp::parse::{build_symbol_hierarchy, prune_outline_depth};
use crate::lsp::read_line_text;
use crate::lsp::rename::build_rename_diff;
use crate::render::{
    render_location_output, render_outline_output, render_rename_output, render_symbol_at_output,
    render_workspace_symbol_output,
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
    let response = match request {
        DaemonRequest::Ping => DaemonWireResponse {
            ok: true,
            payload: Some(json!({
                "pid": std::process::id(),
                "workspace_root": workspace_root,
            })),
            text: Some(format!("daemon alive: {}", workspace_root.display())),
            error: None,
        },
        DaemonRequest::Shutdown => {
            return Ok(DispatchResult::Shutdown(DaemonWireResponse {
                ok: true,
                payload: Some(json!({
                    "pid": std::process::id(),
                    "workspace_root": workspace_root,
                    "stopped": true,
                })),
                text: Some("daemon shutting down".to_string()),
                error: None,
            }));
        }
        DaemonRequest::Goto {
            file,
            line,
            column,
            target,
            limit,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;

            // Route each goto target through the corresponding LSP request.
            let locations = match target {
                GotoTarget::Definition => {
                    adapter.definition_locations(&file, line, position.requested_column)?
                }
                GotoTarget::Declaration => adapter.request_locations(
                    "textDocument/declaration",
                    &file,
                    line,
                    position.requested_column,
                    false,
                )?,
                GotoTarget::Type => adapter.request_locations(
                    "textDocument/typeDefinition",
                    &file,
                    line,
                    position.requested_column,
                    false,
                )?,
            };

            build_location_response(
                LocationOutput {
                    ok: true,
                    workspace_root: workspace_root.to_path_buf(),
                    position,
                    target: Some(target),
                    locations,
                },
                limit,
            )?
        }
        DaemonRequest::Usages {
            file,
            line,
            column,
            include_declaration,
            limit,
        } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let locations = adapter.reference_locations(
                workspace_root,
                &file,
                line,
                position.requested_column,
                include_declaration,
            )?;

            build_location_response(
                LocationOutput {
                    ok: true,
                    workspace_root: workspace_root.to_path_buf(),
                    position,
                    target: None,
                    locations,
                },
                limit,
            )?
        }
        DaemonRequest::FindSymbol { query, kind, limit } => {
            let symbols = adapter.workspace_symbol(&query)?;
            // Enrich symbols with source snippets for context.
            let symbols = symbols
                .into_iter()
                .map(|mut symbol| {
                    symbol.snippet = read_line_text(&symbol.file, symbol.range.start.line)
                        .ok()
                        .map(|value| value.trim().to_string());
                    symbol
                })
                .collect();
            let payload = WorkspaceSymbolOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                query,
                symbols,
            };
            build_rendered_response(render_workspace_symbol_output(limit, &payload, kind)?)?
        }
        DaemonRequest::Inspect { file, line, column } => {
            let position = adapter.resolve_position(&file, line, column)?;
            let hover = Some(adapter.hover(&file, line, position.requested_column)?);
            let payload = SymbolAtOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                symbol: position.symbol.clone(),
                position,
                hover,
            };

            build_rendered_response(render_symbol_at_output(&payload)?)?
        }
        DaemonRequest::Outline { file, depth, limit } => {
            let symbols = adapter.document_symbols(&file)?;
            // Preserve the full tree for --full and prune only when a depth limit is requested.
            let hierarchy = if let Some(depth) = depth {
                prune_outline_depth(build_symbol_hierarchy(symbols), depth)
            } else {
                build_symbol_hierarchy(symbols)
            };
            let payload = OutlineOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                file,
                depth,
                symbols: hierarchy,
            };

            build_rendered_response(render_outline_output(limit, &payload)?)?
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
            let edits = adapter.rename(&file, line, position.requested_column, &new_name)?;
            let total = edits.len();
            let total_files = edits
                .iter()
                .map(|edit| &edit.file)
                .collect::<HashSet<_>>()
                .len();
            let diff = build_rename_diff(workspace_root, &old_name, edits.as_slice())?;
            let payload = RenameOutput {
                ok: true,
                workspace_root: workspace_root.to_path_buf(),
                position,
                old_name,
                new_name,
                total,
                total_files,
                edits,
                diff: Some(diff),
            };
            DaemonWireResponse {
                ok: true,
                payload: Some(serde_json::to_value(&payload)?),
                text: Some(render_rename_output(&payload)?),
                error: None,
            }
        }
    };

    Ok(DispatchResult::Respond(response))
}

fn build_location_response(
    payload: LocationOutput,
    limit: Option<usize>,
) -> Result<DaemonWireResponse> {
    build_rendered_response(render_location_output(limit, &payload)?)
}

fn build_rendered_response(text: String) -> Result<DaemonWireResponse> {
    Ok(DaemonWireResponse {
        ok: true,
        payload: None,
        text: Some(text),
        error: None,
    })
}
