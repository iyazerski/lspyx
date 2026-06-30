use anyhow::Result;
use std::path::Path;

use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::model::{
    DocumentSymbolNode, LocationOutput, LocationRecord, OutlineOutput, ResolvedPosition,
    SymbolAtOutput, WorkspaceSymbolOutput, WorkspaceSymbolRecord, display_path, symbol_kind_name,
};

pub(crate) fn render_location_output(
    limit: Option<usize>,
    payload: &LocationOutput,
) -> Result<String> {
    let total_items = payload.locations.len();
    let locations = apply_limit(payload.locations.as_slice(), limit);

    if locations.is_empty() {
        return Ok(join_sections(vec![
            no_location_result(payload),
            position_context_section(&payload.workspace_root, &payload.position),
        ]));
    }

    let answer = location_summary(locations.len(), total_items, payload);
    let mut results = Vec::new();

    for (index, location) in locations.iter().enumerate() {
        results.push(format!(
            "{}. {}",
            index + 1,
            format_location(&payload.workspace_root, location)
        ));

        if let Some(snippet) = location.snippet.as_deref() {
            results.push(format_snippet_line(
                "   ",
                location.range.start.line,
                snippet,
            ));
        }
    }

    Ok(join_sections(vec![
        answer,
        position_context_section(&payload.workspace_root, &payload.position),
        section("Results", results),
    ]))
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
    let total_items = symbols.len();

    if let Some(n) = limit {
        symbols.truncate(n);
    }

    if symbols.is_empty() {
        return Ok(join_sections(vec![
            workspace_symbol_summary(symbols.len(), total_items, &payload.query),
            query_context_section(payload.query.as_str(), kind_filter),
        ]));
    }

    let mut results = Vec::new();

    for (index, symbol) in symbols.iter().enumerate() {
        results.push(format!(
            "{}. {} [{}]",
            index + 1,
            symbol.name,
            symbol_kind_name(symbol.kind)
        ));
        results.push(format!(
            "   {}",
            format_workspace_position(
                &payload.workspace_root,
                &symbol.file,
                symbol.range.start.line,
                Some(symbol.range.start.column),
            )
        ));

        if let Some(container) = symbol.container_name.as_deref() {
            results.push(format!("   in {container}"));
        }

        if let Some(snippet) = symbol.snippet.as_deref() {
            results.push(format_snippet_line("   ", symbol.range.start.line, snippet));
        }
    }

    Ok(join_sections(vec![
        workspace_symbol_summary(symbols.len(), total_items, &payload.query),
        query_context_section(payload.query.as_str(), kind_filter),
        section("Results", results),
    ]))
}

pub(crate) fn render_symbol_at_output(payload: &SymbolAtOutput) -> Result<String> {
    match payload.symbol.as_ref() {
        Some(symbol) => {
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

            let mut sections = vec![
                symbol_at_summary(&payload.workspace_root, payload),
                position_context_section(&payload.workspace_root, &payload.position),
                section("Details", details),
            ];

            if let Some(hover) = payload
                .hover
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                sections.push(section("Hover", indent_block(hover, "  ")));
            }

            Ok(join_sections(sections))
        }
        None => Ok(join_sections(vec![
            symbol_at_summary(&payload.workspace_root, payload),
            position_context_section(&payload.workspace_root, &payload.position),
        ])),
    }
}

pub(crate) fn render_outline_output(
    limit: Option<usize>,
    payload: &OutlineOutput,
) -> Result<String> {
    let total_items = payload.symbols.len();
    let symbols = apply_limit(payload.symbols.as_slice(), limit);

    if symbols.is_empty() {
        return Ok(join_sections(vec![
            outline_summary(
                symbols.len(),
                total_items,
                &payload.workspace_root,
                &payload.file,
            ),
            outline_context_section(payload),
        ]));
    }

    let mut tree = Vec::new();
    for symbol in symbols {
        tree.extend(format_outline_tree(symbol, 0));
    }

    Ok(join_sections(vec![
        outline_summary(
            symbols.len(),
            total_items,
            &payload.workspace_root,
            &payload.file,
        ),
        outline_context_section(payload),
        section("Outline", tree),
    ]))
}

fn apply_limit<T>(items: &[T], limit: Option<usize>) -> &[T] {
    match limit {
        Some(n) => &items[..n.min(items.len())],
        None => items,
    }
}

fn position_context_section(workspace_root: &Path, position: &ResolvedPosition) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Requested position: {}",
        format_requested_position(workspace_root, position)
    ));

    if position.resolved_column.is_some()
        && position.resolved_column != Some(position.requested_column)
    {
        lines.push(format!(
            "Resolved position: {}",
            format_resolved_position(workspace_root, position)
        ));
    }

    if let Some(source_line) = position
        .source_line
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        lines.push("Source".to_string());
        lines.push(format_snippet_line("  ", position.line, source_line));
    }

    lines.join("\n")
}

fn query_context_section(query: &str, kind_filter: Option<SymbolKindFilter>) -> String {
    let mut lines = vec![sentence(format!("Query {:?}", query))];

    if let Some(kind_filter) = kind_filter {
        lines.push(format!(
            "Filtered to {} symbols.",
            workspace_kind_name(kind_filter)
        ));
    }

    lines.join("\n")
}

fn outline_context_section(payload: &OutlineOutput) -> String {
    let depth = payload
        .depth
        .map(|value| value.to_string())
        .unwrap_or_else(|| "full".to_string());

    [
        sentence(format!(
            "File {:?}",
            display_path(&payload.workspace_root, &payload.file)
        )),
        sentence(format!("Depth {depth}")),
    ]
    .join("\n")
}

fn format_summary_count(shown: usize, total: usize, singular: &str, plural: &str) -> String {
    if shown == total {
        if total == 1 {
            format!("1 {singular}")
        } else {
            format!("{total} {plural}")
        }
    } else {
        format!("{shown} shown of {total} {plural}")
    }
}

fn location_summary(shown: usize, total: usize, payload: &LocationOutput) -> String {
    if shown == 0 {
        return no_location_result(payload);
    }

    match payload.position.symbol.as_ref() {
        Some(symbol) => match payload.target {
            Some(target) => {
                let (singular, plural) = goto_target_labels(target);
                sentence(format!(
                    "{} for {}",
                    format_summary_count(shown, total, singular, plural),
                    symbol.name
                ))
            }
            None => sentence(format!(
                "{} of {}",
                format_summary_count(shown, total, "usage", "usages"),
                symbol.name
            )),
        },
        None => sentence(format_summary_count(shown, total, "location", "locations")),
    }
}

fn workspace_symbol_summary(shown: usize, total: usize, query: &str) -> String {
    if shown == 0 {
        return sentence(format!("no symbols found for query {:?}", query));
    }

    sentence(format!(
        "{} for query {:?}",
        format_summary_count(shown, total, "symbol", "symbols"),
        query
    ))
}

fn symbol_at_summary(workspace_root: &Path, payload: &SymbolAtOutput) -> String {
    match payload.symbol.as_ref() {
        Some(symbol) => {
            let resolved = format_resolved_position(workspace_root, &payload.position);
            match symbol.kind.map(symbol_kind_name) {
                Some(kind) => sentence(format!("{} is a {} at {}", symbol.name, kind, resolved)),
                None => sentence(format!("{} is at {}", symbol.name, resolved)),
            }
        }
        None => sentence(format!(
            "no symbol found at {}",
            format_requested_position(workspace_root, &payload.position)
        )),
    }
}

fn outline_summary(shown: usize, total: usize, workspace_root: &Path, file: &Path) -> String {
    let display_file = display_path(workspace_root, file);

    if shown == 0 {
        return sentence(format!("no symbols found in {display_file}"));
    }

    sentence(format!(
        "{} in {}",
        format_summary_count(shown, total, "top-level symbol", "top-level symbols"),
        display_file
    ))
}

fn no_location_result(payload: &LocationOutput) -> String {
    let requested = format_requested_position(&payload.workspace_root, &payload.position);

    match payload.position.symbol.as_ref() {
        Some(symbol) => match payload.target {
            Some(target) => sentence(format!(
                "no {} found for symbol {:?} at {}",
                goto_target_name(target),
                symbol.name,
                requested
            )),
            None => sentence(format!(
                "no usages found for symbol {:?} at {}",
                symbol.name, requested
            )),
        },
        None => sentence(format!("no symbol found at {}", requested)),
    }
}

fn goto_target_name(target: GotoTarget) -> &'static str {
    match target {
        GotoTarget::Definition => "definition",
        GotoTarget::Declaration => "declaration",
        GotoTarget::Type => "type definition",
    }
}

fn goto_target_labels(target: GotoTarget) -> (&'static str, &'static str) {
    match target {
        GotoTarget::Definition => ("definition", "definitions"),
        GotoTarget::Declaration => ("declaration", "declarations"),
        GotoTarget::Type => ("type definition", "type definitions"),
    }
}

fn workspace_kind_name(kind: SymbolKindFilter) -> &'static str {
    match kind {
        SymbolKindFilter::Class => "class",
        SymbolKindFilter::Function => "function",
        SymbolKindFilter::Method => "method",
    }
}

fn format_requested_position(workspace_root: &Path, position: &ResolvedPosition) -> String {
    format_workspace_position(
        workspace_root,
        &position.file,
        position.line,
        Some(position.requested_column),
    )
}

fn format_resolved_position(workspace_root: &Path, position: &ResolvedPosition) -> String {
    format_workspace_position(
        workspace_root,
        &position.file,
        position.line,
        position.resolved_column,
    )
}

fn format_location(workspace_root: &Path, location: &LocationRecord) -> String {
    format_workspace_position(
        workspace_root,
        &location.file,
        location.range.start.line,
        Some(location.range.start.column),
    )
}

fn format_workspace_position(
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

fn format_outline_tree(symbol: &DocumentSymbolNode, indent: usize) -> Vec<String> {
    let prefix = "  ".repeat(indent);
    let mut lines = vec![format!(
        "{prefix}- {} [{}] @ {}:{}",
        symbol.name,
        symbol_kind_name(symbol.kind),
        symbol.range.start.line,
        symbol.range.start.column
    )];

    for child in &symbol.children {
        lines.extend(format_outline_tree(child, indent + 1));
    }

    lines
}

fn indent_block(text: &str, prefix: &str) -> Vec<String> {
    text.lines().map(|line| format!("{prefix}{line}")).collect()
}

fn format_snippet_line(prefix: &str, line: usize, snippet: &str) -> String {
    format!("{prefix}{line} | {snippet}")
}

fn section(title: &str, lines: Vec<String>) -> String {
    let mut block = vec![title.to_string()];
    block.extend(lines);
    block.join("\n")
}

fn join_sections(sections: Vec<String>) -> String {
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

fn select_workspace_symbols<'a>(
    query: &str,
    symbols: Vec<&'a WorkspaceSymbolRecord>,
) -> Vec<&'a WorkspaceSymbolRecord> {
    let query_lowercase = query.to_lowercase();
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

    let mut prefix_matches = Vec::new();
    let mut substring_matches = Vec::new();
    let mut other_matches = Vec::new();

    for symbol in symbols {
        let name_lowercase = symbol.name.to_lowercase();
        if name_lowercase.starts_with(query_lowercase.as_str()) {
            prefix_matches.push(symbol);
        } else if name_lowercase.contains(query_lowercase.as_str()) {
            substring_matches.push(symbol);
        } else {
            other_matches.push(symbol);
        }
    }

    prefix_matches
        .into_iter()
        .chain(substring_matches)
        .chain(other_matches)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::model::{PositionRecord, RangeRecord, WorkspaceSymbolRecord};

    use super::select_workspace_symbols;

    fn symbol(name: &str) -> WorkspaceSymbolRecord {
        WorkspaceSymbolRecord {
            name: name.to_string(),
            kind: 5,
            container_name: None,
            file: PathBuf::from("src/app.py"),
            range: RangeRecord {
                start: PositionRecord { line: 1, column: 1 },
                end: PositionRecord { line: 1, column: 1 },
            },
            snippet: None,
        }
    }

    #[test]
    fn exact_case_sensitive_workspace_symbol_matches_win() {
        let symbols = [symbol("order"), symbol("Order"), symbol("OrderManager")];
        let symbols = symbols.iter().collect();

        let selected = select_workspace_symbols("Order", symbols);

        assert_eq!(
            selected
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Order"]
        );
    }

    #[test]
    fn exact_case_insensitive_workspace_symbol_matches_win() {
        let symbols = [symbol("order"), symbol("OrderManager")];
        let symbols = symbols.iter().collect();

        let selected = select_workspace_symbols("Order", symbols);

        assert_eq!(
            selected
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["order"]
        );
    }

    #[test]
    fn prefix_workspace_symbol_matches_rank_before_substrings() {
        let symbols = [
            symbol("TestOrderFlow"),
            symbol("OrderManager"),
            symbol("PreOrder"),
            symbol("OrderPolicy"),
            symbol("execute_orders"),
        ];
        let symbols = symbols.iter().collect();

        let selected = select_workspace_symbols("Order", symbols);

        assert_eq!(
            selected
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "OrderManager",
                "OrderPolicy",
                "TestOrderFlow",
                "PreOrder",
                "execute_orders",
            ]
        );
    }
}
