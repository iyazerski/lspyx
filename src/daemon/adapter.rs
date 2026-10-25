use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::Declaration;
use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::file_position::FilePosition;
use crate::lsp::model::{
    DocumentSymbolNode, LocationRecord, PositionRecord, RangeRecord, RenameEditRecord,
    ResolvedPosition, SymbolAtRecord, WorkspaceSymbolRecord,
};
use crate::lsp::parse::{
    extract_symbol_at, find_document_symbol, is_symbol_char, parse_document_symbols,
    parse_hover_contents, parse_location_response, parse_workspace_edit, parse_workspace_symbols,
};
use crate::lsp::{LspSession, SnippetReader, column_to_utf16_offset, nth_line, path_to_file_uri};
use crate::workspace::{is_python_file, is_walkable_dir, read_dir};

/// A long-lived ty LSP session that keeps request files synced as open documents.
#[derive(Debug)]
pub(super) struct PersistentAdapter {
    session: LspSession,
    /// Open documents keyed by their canonical path; request files are canonicalized by clients.
    documents: HashMap<PathBuf, OpenDocument>,
}

#[derive(Debug)]
struct OpenDocument {
    text: String,
    version: i32,
}

impl PersistentAdapter {
    pub(super) fn new(workspace_root: &Path, ty_binary: &Path) -> Result<Self> {
        Ok(Self {
            session: LspSession::start(ty_binary, workspace_root)?,
            documents: HashMap::new(),
        })
    }

    /// Shut down the LSP session.
    pub(super) fn shutdown(self) {
        self.session.shutdown();
    }

    /// Identify the symbol at `position`, enriched with its document symbol kind and detail.
    pub(super) fn resolve_position(&mut self, position: &FilePosition) -> Result<ResolvedPosition> {
        let FilePosition { file, line, column } = position;
        self.ensure_file_synced(file)?;
        let line_text = self.document_line(file, *line)?;
        let source_line = line_text.trim().to_string();
        let symbol = extract_symbol_at(line_text, *column)?;
        let resolved_column = symbol.as_ref().map(|value| value.start_column);
        let symbol = symbol
            .map(|symbol| self.describe_symbol(file, *line, symbol))
            .transpose()?;

        Ok(ResolvedPosition {
            file: file.clone(),
            line: *line,
            requested_column: *column,
            resolved_column,
            source_line,
            symbol,
        })
    }

    /// Add kind and detail from the matching document symbol, when there is one.
    fn describe_symbol(
        &mut self,
        file: &Path,
        line: usize,
        symbol: SymbolAtRecord,
    ) -> Result<SymbolAtRecord> {
        let document_symbols = self.document_symbols(file)?;
        let position = PositionRecord {
            line,
            column: symbol.start_column,
        };
        Ok(
            match find_document_symbol(&document_symbols, position)
                .filter(|value| value.name == symbol.name)
            {
                Some(document_symbol) => SymbolAtRecord {
                    kind: Some(document_symbol.kind),
                    detail: document_symbol.detail.clone(),
                    ..symbol
                },
                None => symbol,
            },
        )
    }

    pub(super) fn goto_locations(
        &mut self,
        position: &ResolvedPosition,
        target: GotoTarget,
    ) -> Result<Vec<LocationRecord>> {
        match target {
            GotoTarget::Definition => self.definition_locations(position),
            GotoTarget::Declaration => {
                let params = self.position_params(position)?;
                self.locations("textDocument/declaration", params)
            }
            GotoTarget::Type => {
                let params = self.position_params(position)?;
                self.locations("textDocument/typeDefinition", params)
            }
        }
    }

    pub(super) fn definition_locations(
        &mut self,
        position: &ResolvedPosition,
    ) -> Result<Vec<LocationRecord>> {
        let params = self.position_params(position)?;
        let locations = self.locations("textDocument/definition", params)?;
        let Some(symbol) = position.symbol.as_ref() else {
            return Ok(locations);
        };

        // Follow re-exports: when ty stops at an import line, jump to the unique workspace symbol.
        let redirected = locations
            .iter()
            .any(|location| is_import_location(location, &symbol.name));
        if !redirected {
            return Ok(locations);
        }
        Ok(match self.unique_workspace_symbol(&symbol.name)? {
            Some(unique) => vec![LocationRecord {
                snippet: SnippetReader::default().snippet(&unique.file, unique.range.start.line),
                file: unique.file,
                range: unique.range,
            }],
            None => locations,
        })
    }

    pub(super) fn reference_locations(
        &mut self,
        workspace_root: &Path,
        position: &ResolvedPosition,
        declaration: Declaration,
    ) -> Result<Vec<LocationRecord>> {
        let mut params = self.position_params(position)?;
        params["context"] = json!({ "includeDeclaration": declaration == Declaration::Include });
        let locations = self.locations("textDocument/references", params)?;

        let unique_files: HashSet<&PathBuf> =
            locations.iter().map(|location| &location.file).collect();
        if unique_files.len() > 1 {
            return Ok(locations);
        }

        // ty may only report same-file references; widen with a lexical scan for unique symbols.
        let Some(symbol) = position.symbol.as_ref() else {
            return Ok(locations);
        };
        let Some(canonical_symbol) = self.unique_workspace_symbol(&symbol.name)? else {
            return Ok(locations);
        };

        let mut lexical_locations = Vec::new();
        collect_symbol_occurrences(
            workspace_root,
            &canonical_symbol.name,
            &mut lexical_locations,
        )?;
        Ok(merge_locations(locations, lexical_locations))
    }

    pub(super) fn hover(&mut self, position: &ResolvedPosition) -> Result<String> {
        let params = self.position_params(position)?;
        parse_hover_contents(self.session.request("textDocument/hover", params)?)
    }

    pub(super) fn document_symbols(&mut self, file: &Path) -> Result<Vec<DocumentSymbolNode>> {
        self.ensure_file_synced(file)?;
        let response = self.session.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": path_to_file_uri(file)? } }),
        )?;
        parse_document_symbols(response)
    }

    /// Search workspace symbols, keep the best matches, and return them with the match total.
    pub(super) fn search_workspace_symbols(
        &mut self,
        query: &str,
        kind: Option<SymbolKindFilter>,
        limit: Option<usize>,
    ) -> Result<(Vec<WorkspaceSymbolRecord>, usize)> {
        let mut symbols = self.workspace_symbol(query)?;
        if let Some(kind) = kind {
            symbols.retain(|symbol| symbol.kind == kind.lsp_kind());
        }
        let mut symbols = rank_workspace_symbols(query, symbols);
        let total = symbols.len();
        if let Some(limit) = limit {
            symbols.truncate(limit);
        }

        // Read snippets only for the symbols that will be shown.
        let mut snippets = SnippetReader::default();
        for symbol in &mut symbols {
            symbol.snippet = snippets.snippet(&symbol.file, symbol.range.start.line);
        }
        Ok((symbols, total))
    }

    pub(super) fn rename(
        &mut self,
        position: &ResolvedPosition,
        new_name: &str,
    ) -> Result<Vec<RenameEditRecord>> {
        let mut params = self.position_params(position)?;
        params["newName"] = json!(new_name);
        parse_workspace_edit(self.session.request("textDocument/rename", params)?)
    }

    fn workspace_symbol(&mut self, query: &str) -> Result<Vec<WorkspaceSymbolRecord>> {
        let response = self
            .session
            .request("workspace/symbol", json!({ "query": query }))?;
        parse_workspace_symbols(response)
    }

    fn unique_workspace_symbol(&mut self, query: &str) -> Result<Option<WorkspaceSymbolRecord>> {
        let symbols = self.workspace_symbol(query)?;
        Ok(single_match(&symbols, |symbol| symbol.name == query)
            .or_else(|| single_match(&symbols, |symbol| symbol.name.eq_ignore_ascii_case(query))))
    }

    /// Send a location request and attach source snippets to the results.
    fn locations(&mut self, method: &str, params: Value) -> Result<Vec<LocationRecord>> {
        let mut locations = parse_location_response(self.session.request(method, params)?)?;
        let mut snippets = SnippetReader::default();
        for location in &mut locations {
            location.snippet = snippets.snippet(&location.file, location.range.start.line);
        }
        Ok(locations)
    }

    /// Sync the position's file and build `TextDocumentPositionParams` for it.
    fn position_params(&mut self, position: &ResolvedPosition) -> Result<Value> {
        self.ensure_file_synced(&position.file)?;
        let line_text = self.document_line(&position.file, position.line)?;
        let character = column_to_utf16_offset(line_text, position.requested_column)?;
        Ok(json!({
            "textDocument": { "uri": path_to_file_uri(&position.file)? },
            "position": { "line": position.line - 1, "character": character },
        }))
    }

    fn ensure_file_synced(&mut self, file: &Path) -> Result<()> {
        let text = fs::read_to_string(file)
            .with_context(|| format!("failed to read {}", file.display()))?;

        match self.documents.get_mut(file) {
            Some(document) if document.text != text => {
                document.version += 1;
                self.session.change_file(file, document.version, &text)?;
                document.text = text;
            }
            Some(_) => {}
            None => {
                self.session.open_file_with_text(file, 1, &text)?;
                self.documents
                    .insert(file.to_path_buf(), OpenDocument { text, version: 1 });
            }
        }

        Ok(())
    }

    /// Return a 1-based line from the synced text of `file`.
    fn document_line(&self, file: &Path, line: usize) -> Result<&str> {
        let document = self
            .documents
            .get(file)
            .with_context(|| format!("{} is not synced", file.display()))?;
        nth_line(&document.text, line)
            .with_context(|| format!("line {line} is out of range for {}", file.display()))
    }
}

/// Return the only item matching `predicate`, or `None` when there are zero or several.
fn single_match<T: Clone>(items: &[T], predicate: impl Fn(&T) -> bool) -> Option<T> {
    let mut matches = items.iter().filter(|item| predicate(item));
    let first = matches.next()?;
    matches.next().is_none().then(|| first.clone())
}

/// Rank symbols: exact matches win outright, then prefix, substring, and other matches.
fn rank_workspace_symbols(
    query: &str,
    mut symbols: Vec<WorkspaceSymbolRecord>,
) -> Vec<WorkspaceSymbolRecord> {
    let exact_matchers: [fn(&str, &str) -> bool; 2] = [
        |name, query| name == query,
        |name, query| name.eq_ignore_ascii_case(query),
    ];
    for exact in exact_matchers {
        if symbols.iter().any(|symbol| exact(&symbol.name, query)) {
            symbols.retain(|symbol| exact(&symbol.name, query));
            return symbols;
        }
    }

    let query = query.to_lowercase();
    // The cached-key sort is stable, so server order is kept within each rank.
    symbols.sort_by_cached_key(|symbol| {
        let name = symbol.name.to_lowercase();
        if name.starts_with(&query) {
            0
        } else if name.contains(&query) {
            1
        } else {
            2
        }
    });
    symbols
}

fn is_import_location(location: &LocationRecord, symbol_name: &str) -> bool {
    location.snippet.as_deref().is_some_and(|snippet| {
        (snippet.starts_with("from ") || snippet.starts_with("import "))
            && snippet.contains(symbol_name)
    })
}

fn collect_symbol_occurrences(
    directory: &Path,
    symbol_name: &str,
    locations: &mut Vec<LocationRecord>,
) -> Result<()> {
    for entry in read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();

        if is_walkable_dir(&entry)? {
            collect_symbol_occurrences(&path, symbol_name, locations)?;
            continue;
        }
        if !entry.file_type()?.is_file() || !is_python_file(&path) {
            continue;
        }

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        // Skip the per-line scan for the common case of files that never mention the symbol.
        if !text.contains(symbol_name) {
            continue;
        }

        let symbol_width = symbol_name.chars().count();
        for (line_index, line) in text.lines().enumerate() {
            for column in symbol_columns(line, symbol_name) {
                let start = PositionRecord {
                    line: line_index + 1,
                    column,
                };
                locations.push(LocationRecord {
                    file: path.clone(),
                    range: RangeRecord {
                        start,
                        end: PositionRecord {
                            column: column + symbol_width,
                            ..start
                        },
                    },
                    snippet: Some(line.trim().to_string()),
                });
            }
        }
    }

    Ok(())
}

/// Yield 1-based character columns where `symbol_name` occurs as a whole identifier.
fn symbol_columns<'a>(line: &'a str, symbol_name: &'a str) -> impl Iterator<Item = usize> + 'a {
    line.match_indices(symbol_name)
        .filter(move |(byte_index, _)| {
            let before = line[..*byte_index].chars().next_back();
            let after = line[byte_index + symbol_name.len()..].chars().next();
            !before.is_some_and(is_symbol_char) && !after.is_some_and(is_symbol_char)
        })
        .map(|(byte_index, _)| line[..byte_index].chars().count() + 1)
}

fn merge_locations(
    mut primary: Vec<LocationRecord>,
    additional: Vec<LocationRecord>,
) -> Vec<LocationRecord> {
    let mut seen: HashSet<(PathBuf, PositionRecord)> = primary
        .iter()
        .map(|location| (location.file.clone(), location.range.start))
        .collect();

    for location in additional {
        if seen.insert((location.file.clone(), location.range.start)) {
            primary.push(location);
        }
    }

    primary
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::lsp::model::{PositionRecord, RangeRecord, SymbolKind, WorkspaceSymbolRecord};

    use super::{rank_workspace_symbols, symbol_columns};

    fn symbol(name: &str) -> WorkspaceSymbolRecord {
        let position = PositionRecord { line: 1, column: 1 };
        WorkspaceSymbolRecord {
            name: name.to_string(),
            kind: SymbolKind::CLASS,
            container_name: None,
            file: PathBuf::from("src/app.py"),
            range: RangeRecord {
                start: position,
                end: position,
            },
            snippet: None,
        }
    }

    fn ranked(query: &str, names: &[&str]) -> Vec<String> {
        rank_workspace_symbols(query, names.iter().map(|name| symbol(name)).collect())
            .into_iter()
            .map(|symbol| symbol.name)
            .collect()
    }

    #[test]
    fn exact_case_sensitive_workspace_symbol_matches_win() {
        assert_eq!(
            ranked("Order", &["order", "Order", "OrderManager"]),
            vec!["Order"]
        );
    }

    #[test]
    fn exact_case_insensitive_workspace_symbol_matches_win() {
        assert_eq!(ranked("Order", &["order", "OrderManager"]), vec!["order"]);
    }

    #[test]
    fn prefix_workspace_symbol_matches_rank_before_substrings() {
        assert_eq!(
            ranked(
                "Order",
                &[
                    "TestOrderFlow",
                    "OrderManager",
                    "PreOrder",
                    "OrderPolicy",
                    "execute_orders",
                ]
            ),
            vec![
                "OrderManager",
                "OrderPolicy",
                "TestOrderFlow",
                "PreOrder",
                "execute_orders",
            ]
        );
    }

    #[test]
    fn symbol_columns_match_whole_identifiers_only() {
        assert_eq!(
            symbol_columns("Chat(ChatRoom, é.Chat)", "Chat").collect::<Vec<_>>(),
            vec![1, 18]
        );
    }
}
