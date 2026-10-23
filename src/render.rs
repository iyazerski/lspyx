use std::path::Path;

use crate::cli::GotoTarget;
use crate::lsp::model::{
    DocumentSymbolNode, LocationOutput, OutlineOutput, RenameOutput, ResolvedPosition,
    SymbolAtOutput, WorkspaceSymbolOutput, display_path, symbol_kind_name,
};

pub(crate) fn render_location_output(limit: Option<usize>, payload: &LocationOutput) -> String {
    let locations = apply_limit(&payload.locations, limit);
    let mut results = Vec::new();

    for (index, location) in locations.iter().enumerate() {
        let start = location.range.start;
        results.push(format!(
            "{}. {}",
            index + 1,
            format_position(
                &payload.workspace_root,
                &location.file,
                start.line,
                Some(start.column)
            )
        ));
        if let Some(snippet) = location.snippet.as_deref() {
            results.push(format_snippet_line("   ", start.line, snippet));
        }
    }

    join_sections([
        location_summary(locations.len(), payload.locations.len(), payload),
        position_context_section(&payload.workspace_root, &payload.position),
        section("Results", results),
    ])
}

pub(crate) fn render_workspace_symbol_output(payload: &WorkspaceSymbolOutput) -> String {
    let mut results = Vec::new();

    for (index, symbol) in payload.symbols.iter().enumerate() {
        let start = symbol.range.start;
        results.push(format!(
            "{}. {} [{}]",
            index + 1,
            symbol.name,
            symbol_kind_name(symbol.kind)
        ));
        results.push(format!(
            "   {}",
            format_position(
                &payload.workspace_root,
                &symbol.file,
                start.line,
                Some(start.column)
            )
        ));
        if let Some(container) = symbol.container_name.as_deref() {
            results.push(format!("   in {container}"));
        }
        if let Some(snippet) = symbol.snippet.as_deref() {
            results.push(format_snippet_line("   ", start.line, snippet));
        }
    }

    let summary = if payload.symbols.is_empty() {
        format!("no symbols found for query {:?}", payload.query)
    } else {
        format!(
            "{} for query {:?}",
            format_summary_count(payload.symbols.len(), payload.total, "symbol", "symbols"),
            payload.query
        )
    };
    let mut context = vec![sentence(format!("Query {:?}", payload.query))];
    if let Some(kind) = payload.kind {
        context.push(format!(
            "Filtered to {} symbols.",
            symbol_kind_name(kind.lsp_kind())
        ));
    }

    join_sections([
        sentence(summary),
        context.join("\n"),
        section("Results", results),
    ])
}

pub(crate) fn render_symbol_at_output(payload: &SymbolAtOutput) -> String {
    let position = &payload.position;
    let workspace_root = &payload.workspace_root;
    let Some(symbol) = position.symbol.as_ref() else {
        return join_sections([
            sentence(format!(
                "no symbol found at {}",
                format_requested_position(workspace_root, position)
            )),
            position_context_section(workspace_root, position),
        ]);
    };

    let resolved = format_resolved_position(workspace_root, position);
    let summary = match symbol.kind.map(symbol_kind_name) {
        Some(kind) => format!("{} is a {kind} at {resolved}", symbol.name),
        None => format!("{} is at {resolved}", symbol.name),
    };

    let mut details = Vec::new();
    if let Some(kind) = symbol.kind {
        details.push(format!("Kind: {}", symbol_kind_name(kind)));
    }
    details.push(format!(
        "Range: columns {}-{}",
        symbol.start_column, symbol.end_column
    ));
    if let Some(detail) = symbol.detail.as_deref() {
        details.push(format!("Detail: {detail}"));
    }

    let hover = if payload.hover.trim().is_empty() {
        Vec::new()
    } else {
        payload
            .hover
            .lines()
            .map(|line| format!("  {line}"))
            .collect()
    };

    join_sections([
        sentence(summary),
        position_context_section(workspace_root, position),
        section("Details", details),
        section("Hover", hover),
    ])
}

pub(crate) fn render_outline_output(limit: Option<usize>, payload: &OutlineOutput) -> String {
    let symbols = apply_limit(&payload.symbols, limit);
    let display_file = display_path(&payload.workspace_root, &payload.file);

    let summary = if symbols.is_empty() {
        format!("no symbols found in {display_file}")
    } else {
        format!(
            "{} in {display_file}",
            format_summary_count(
                symbols.len(),
                payload.symbols.len(),
                "top-level symbol",
                "top-level symbols"
            )
        )
    };
    let depth = payload
        .depth
        .map_or_else(|| "full".to_string(), |value| value.to_string());
    let context = format!(
        "{}\n{}",
        sentence(format!("File {display_file:?}")),
        sentence(format!("Depth {depth}"))
    );

    let mut tree = Vec::new();
    for symbol in symbols {
        push_outline_tree(&mut tree, symbol, 0);
    }

    join_sections([sentence(summary), context, section("Outline", tree)])
}

pub(crate) fn render_rename_output(payload: &RenameOutput) -> String {
    let summary = if payload.total == 0 {
        format!(
            "no edits found to rename {:?} to {:?}",
            payload.old_name, payload.new_name
        )
    } else {
        format!(
            "{} across {} to rename {:?} to {:?}",
            format_summary_count(payload.total, payload.total, "edit", "edits"),
            format_summary_count(payload.total_files, payload.total_files, "file", "files"),
            payload.old_name,
            payload.new_name,
        )
    };

    join_sections([
        sentence(summary),
        position_context_section(&payload.workspace_root, &payload.position),
        section(
            "Unified diff",
            payload.diff.lines().map(str::to_string).collect(),
        ),
    ])
}

/// Combine titled renderings into one markdown response.
pub(crate) fn render_position_bundle(sections: &[(&str, String)]) -> String {
    sections
        .iter()
        .map(|(title, body)| format!("## {title}\n\n{body}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn apply_limit<T>(items: &[T], limit: Option<usize>) -> &[T] {
    &items[..limit.map_or(items.len(), |limit| limit.min(items.len()))]
}

fn position_context_section(workspace_root: &Path, position: &ResolvedPosition) -> String {
    let mut lines = vec![format!(
        "Requested position: {}",
        format_requested_position(workspace_root, position)
    )];

    if position
        .resolved_column
        .is_some_and(|column| column != position.requested_column)
    {
        lines.push(format!(
            "Resolved position: {}",
            format_resolved_position(workspace_root, position)
        ));
    }

    if !position.source_line.is_empty() {
        lines.push("Source".to_string());
        lines.push(format_snippet_line(
            "  ",
            position.line,
            &position.source_line,
        ));
    }

    lines.join("\n")
}

fn format_summary_count(shown: usize, total: usize, singular: &str, plural: &str) -> String {
    match (shown == total, total) {
        (true, 1) => format!("1 {singular}"),
        (true, _) => format!("{total} {plural}"),
        (false, _) => format!("{shown} shown of {total} {plural}"),
    }
}

fn location_summary(shown: usize, total: usize, payload: &LocationOutput) -> String {
    let requested = format_requested_position(&payload.workspace_root, &payload.position);
    let Some(symbol) = payload.position.symbol.as_ref() else {
        return sentence(if shown == 0 {
            format!("no symbol found at {requested}")
        } else {
            format_summary_count(shown, total, "location", "locations")
        });
    };

    let (singular, plural) = match payload.target {
        Some(GotoTarget::Definition) => ("definition", "definitions"),
        Some(GotoTarget::Declaration) => ("declaration", "declarations"),
        Some(GotoTarget::Type) => ("type definition", "type definitions"),
        None => ("usage", "usages"),
    };
    sentence(match (shown, payload.target) {
        (0, Some(_)) => format!(
            "no {singular} found for symbol {:?} at {requested}",
            symbol.name
        ),
        (0, None) => format!(
            "no usages found for symbol {:?} at {requested}",
            symbol.name
        ),
        (_, Some(_)) => format!(
            "{} for {}",
            format_summary_count(shown, total, singular, plural),
            symbol.name
        ),
        (_, None) => format!(
            "{} of {}",
            format_summary_count(shown, total, singular, plural),
            symbol.name
        ),
    })
}

fn format_requested_position(workspace_root: &Path, position: &ResolvedPosition) -> String {
    format_position(
        workspace_root,
        &position.file,
        position.line,
        Some(position.requested_column),
    )
}

fn format_resolved_position(workspace_root: &Path, position: &ResolvedPosition) -> String {
    format_position(
        workspace_root,
        &position.file,
        position.line,
        position.resolved_column,
    )
}

fn format_position(
    workspace_root: &Path,
    file: &Path,
    line: usize,
    column: Option<usize>,
) -> String {
    let file = display_path(workspace_root, file);
    match column {
        Some(column) => format!("{file}:{line}:{column}"),
        None => format!("{file}:{line}"),
    }
}

fn push_outline_tree(lines: &mut Vec<String>, symbol: &DocumentSymbolNode, indent: usize) {
    lines.push(format!(
        "{}- {} [{}] @ {}:{}",
        "  ".repeat(indent),
        symbol.name,
        symbol_kind_name(symbol.kind),
        symbol.range.start.line,
        symbol.range.start.column
    ));
    for child in &symbol.children {
        push_outline_tree(lines, child, indent + 1);
    }
}

fn format_snippet_line(prefix: &str, line: usize, snippet: &str) -> String {
    format!("{prefix}{line} | {snippet}")
}

/// Render a titled block, or an empty string (dropped by `join_sections`) when it has no lines.
fn section(title: &str, lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    format!("{title}\n{}", lines.join("\n"))
}

fn join_sections(sections: impl IntoIterator<Item = String>) -> String {
    sections
        .into_iter()
        .filter(|section| !section.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn sentence(text: String) -> String {
    if text.ends_with('.') {
        text
    } else {
        format!("{text}.")
    }
}
