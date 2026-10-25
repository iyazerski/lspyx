use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use super::adapter::PersistentAdapter;
use super::{DaemonReply, DaemonRequest, Declaration};
use crate::cli::GotoTarget;
use crate::lsp::model::{
    LocationOutput, LocationRecord, OutlineDepth, OutlineOutput, RenameOutput, ResolvedPosition,
    SymbolAtOutput, WorkspaceSymbolOutput,
};
use crate::lsp::parse::{build_symbol_hierarchy, prune_outline_depth};
use crate::lsp::rename::build_rename_diff;
use crate::render::{
    render_location_output, render_outline_output, render_position_bundle, render_rename_output,
    render_symbol_at_output, render_workspace_symbol_output,
};

#[derive(Debug)]
pub(super) enum DispatchResult {
    Respond(DaemonReply),
    /// Send the reply, then stop the daemon.
    Shutdown(DaemonReply),
}

/// Run one daemon request against the persistent LSP adapter.
pub(super) fn dispatch_request(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    request: DaemonRequest,
) -> Result<DispatchResult> {
    let pid_payload = || json!({ "pid": std::process::id() });
    let text = match request {
        DaemonRequest::Ping => {
            return Ok(DispatchResult::Respond(DaemonReply::with_payload(
                format!("daemon alive: {}", workspace_root.display()),
                pid_payload(),
            )));
        }
        DaemonRequest::Shutdown => {
            return Ok(DispatchResult::Shutdown(DaemonReply::with_payload(
                "daemon shutting down".to_string(),
                pid_payload(),
            )));
        }
        DaemonRequest::Goto {
            position,
            target,
            limit,
        } => {
            let position = adapter.resolve_position(&position)?;
            let locations = adapter.goto_locations(&position, target)?;
            render_locations(workspace_root, position, Some(target), locations, limit)
        }
        DaemonRequest::Usages {
            position,
            declaration,
            limit,
        } => {
            let position = adapter.resolve_position(&position)?;
            let locations = adapter.reference_locations(workspace_root, &position, declaration)?;
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
        DaemonRequest::Inspect { position } => {
            let position = adapter.resolve_position(&position)?;
            inspect(workspace_root, adapter, position)?
        }
        DaemonRequest::Explore { position, limit } => {
            // Resolve once and reuse the position for every section of the bundle.
            let position = adapter.resolve_position(&position)?;
            explore(workspace_root, adapter, position, limit)?
        }
        DaemonRequest::Outline { file, depth, limit } => {
            let symbols = build_symbol_hierarchy(adapter.document_symbols(&file)?);
            let symbols = match depth {
                OutlineDepth::Limited(depth) => prune_outline_depth(symbols, depth),
                OutlineDepth::Full => symbols,
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
        DaemonRequest::Rename { position, new_name } => {
            let position = adapter.resolve_position(&position)?;
            let payload = rename(workspace_root, adapter, position, new_name)?;
            return Ok(DispatchResult::Respond(DaemonReply::with_payload(
                render_rename_output(&payload),
                serde_json::to_value(&payload)?,
            )));
        }
    };

    Ok(DispatchResult::Respond(DaemonReply::text(text)))
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

/// Render the inspection, definitions, and usages of the symbol at `position` as one bundle.
fn explore(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    position: ResolvedPosition,
    limit: Option<usize>,
) -> Result<String> {
    if position.symbol.is_none() {
        let inspect_text = inspect(workspace_root, adapter, position)?;
        return Ok(render_position_bundle(&[("Inspect", inspect_text)]));
    }

    // Each rendered section owns its position, so the copies are deliberate.
    let inspect_text = inspect(workspace_root, adapter, position.clone())?;
    let definitions = adapter.definition_locations(&position)?;
    let usages = adapter.reference_locations(workspace_root, &position, Declaration::Include)?;
    let definition_text = render_locations(
        workspace_root,
        position.clone(),
        Some(GotoTarget::Definition),
        definitions,
        limit,
    );
    let usages_text = render_locations(workspace_root, position, None, usages, limit);
    Ok(render_position_bundle(&[
        ("Inspect", inspect_text),
        ("Definition", definition_text),
        ("Usages", usages_text),
    ]))
}

/// Preview renaming the symbol at `position` to `new_name` as a diff with structured edits.
fn rename(
    workspace_root: &Path,
    adapter: &mut PersistentAdapter,
    position: ResolvedPosition,
    new_name: String,
) -> Result<RenameOutput> {
    let old_name = position
        .symbol
        .as_ref()
        .map(|symbol| symbol.name.clone())
        .context("no symbol found at the requested rename position")?;
    let edits = adapter.rename(&position, &new_name)?;
    Ok(RenameOutput {
        workspace_root: workspace_root.to_path_buf(),
        diff: build_rename_diff(workspace_root, &old_name, &edits)?,
        position,
        old_name,
        new_name,
        total: edits.len(),
        // Edits are sorted by file, so adjacent grouping counts distinct files.
        total_files: edits
            .chunk_by(|left, right| left.file == right.file)
            .count(),
        edits,
    })
}

fn render_locations(
    workspace_root: &Path,
    position: ResolvedPosition,
    target: Option<GotoTarget>,
    locations: Vec<LocationRecord>,
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
