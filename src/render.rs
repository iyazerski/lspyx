use anyhow::Result;
use std::path::Path;

use crate::cli::SymbolKindFilter;
use crate::model::{
    DocumentSymbolNode, LocationOutput, LocationRecord, OutlineOutput, SymbolAtOutput,
    WorkspaceSymbolOutput, WorkspaceSymbolRecord, display_path, symbol_kind_name,
};

pub(crate) fn render_location_output(
    limit: Option<usize>,
    payload: &LocationOutput,
) -> Result<String> {
    let locations = apply_limit(payload.locations.as_slice(), limit);
    Ok(format_locations_text(&payload.workspace_root, locations))
}

pub(crate) fn render_workspace_symbol_output(
    limit: Option<usize>,
    payload: &WorkspaceSymbolOutput,
    kind_filter: Option<SymbolKindFilter>,
) -> Result<String> {
    let symbols = payload
        .symbols
        .iter()
        .filter(|symbol| kind_filter.is_none_or(|filter| filter.matches(symbol.kind)))
        .collect::<Vec<_>>();
    let mut symbols = select_workspace_symbols(payload.query.as_str(), symbols);

    if let Some(n) = limit {
        symbols.truncate(n);
    }

    Ok(if symbols.is_empty() {
        "no results".to_string()
    } else {
        symbols
            .iter()
            .map(|symbol| {
                let container = symbol
                    .container_name
                    .as_deref()
                    .map(|name| format!("  ({name})"))
                    .unwrap_or_default();
                let snippet = symbol
                    .snippet
                    .as_deref()
                    .map(|s| format!("\n  {}: {s}", symbol.range.start.line))
                    .unwrap_or_default();
                format!(
                    "{} [{}] {}:{}:{}{}{}",
                    symbol.name,
                    symbol_kind_name(symbol.kind),
                    display_path(&payload.workspace_root, &symbol.file),
                    symbol.range.start.line,
                    symbol.range.start.column,
                    container,
                    snippet
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

pub(crate) fn render_symbol_at_output(payload: &SymbolAtOutput) -> Result<String> {
    let symbol_text = payload
        .symbol
        .as_ref()
        .map(|symbol| symbol.name.clone())
        .unwrap_or_else(|| "no symbol".to_string());
    let hover_text = payload.hover.as_deref().unwrap_or("");

    Ok(format!(
        "{}:{}:{}\n{}\n{}",
        display_path(&payload.workspace_root, &payload.file),
        payload.line,
        payload.column,
        symbol_text,
        hover_text
    ))
}

pub(crate) fn render_outline_output(
    limit: Option<usize>,
    payload: &OutlineOutput,
) -> Result<String> {
    let symbols = apply_limit(payload.symbols.as_slice(), limit);

    Ok(symbols
        .iter()
        .map(DocumentSymbolNode::format_text)
        .collect::<Vec<_>>()
        .join("\n"))
}

fn apply_limit<T>(items: &[T], limit: Option<usize>) -> &[T] {
    match limit {
        Some(n) => &items[..n.min(items.len())],
        None => items,
    }
}

fn select_workspace_symbols<'a>(
    query: &str,
    symbols: Vec<&'a WorkspaceSymbolRecord>,
) -> Vec<&'a WorkspaceSymbolRecord> {
    let exact_case_sensitive = symbols
        .iter()
        .copied()
        .filter(|symbol| symbol.name == query)
        .collect::<Vec<_>>();
    if !exact_case_sensitive.is_empty() {
        return exact_case_sensitive;
    }

    let exact_case_insensitive = symbols
        .iter()
        .copied()
        .filter(|symbol| symbol.name.eq_ignore_ascii_case(query))
        .collect::<Vec<_>>();
    if !exact_case_insensitive.is_empty() {
        return exact_case_insensitive;
    }

    symbols
}

fn format_locations_text(workspace_root: &Path, locations: &[LocationRecord]) -> String {
    if locations.is_empty() {
        return "no results".to_string();
    }

    locations
        .iter()
        .map(|location| {
            let snippet = location.snippet.as_ref().map_or(String::new(), |value| {
                format!("\n  {}: {value}", location.range.start.line)
            });

            format!(
                "{}:{}:{}{}",
                display_path(workspace_root, &location.file),
                location.range.start.line,
                location.range.start.column,
                snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
