use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::Declaration;
use super::watch::WorkspaceWatcher;
use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::file_position::FilePosition;
use crate::lsp::model::{
    DocumentSymbolNode, LocationRecord, PositionRecord, RenameEditRecord, ResolvedPosition,
    SymbolAtRecord, WorkspaceSymbolRecord,
};
use crate::lsp::parse::{
    extract_symbol_at, find_document_symbol, parse_document_symbols, parse_hover_contents,
    parse_location_response, parse_workspace_edit, parse_workspace_symbols,
};
use crate::lsp::{
    FileChangeType, LspSession, SnippetReader, column_to_utf16_offset, nth_line, path_to_file_uri,
};

/// A long-lived ty LSP session that keeps request files synced as open documents.
#[derive(Debug)]
pub(super) struct PersistentAdapter {
    session: LspSession,
    watcher: WorkspaceWatcher,
    /// Set once forwarding disk changes failed, so ty's view of the disk can't be trusted.
    sync_failed: bool,
    /// Open documents keyed by their canonical path; request files are canonicalized by clients.
    documents: HashMap<PathBuf, OpenDocument>,
}

#[derive(Debug)]
struct OpenDocument {
    text: String,
    version: i32,
}

impl PersistentAdapter {
    pub(super) fn new(
        workspace_root: &Path,
        ty_binary: &Path,
        cookie_prefix: &Path,
    ) -> Result<Self> {
        // Watch before ty reads the workspace, so no edit falls between the two.
        let watcher = WorkspaceWatcher::new(workspace_root, cookie_prefix)?;
        Ok(Self {
            session: LspSession::start(ty_binary, workspace_root)?,
            watcher,
            sync_failed: false,
            documents: HashMap::new(),
        })
    }

    /// Shut down the LSP session.
    pub(super) fn shutdown(self) {
        self.session.shutdown();
    }

    /// Fail when the ty session or the file watcher can no longer give correct answers.
    ///
    /// # Errors
    ///
    /// Returns an error describing the broken part.
    pub(super) fn ensure_healthy(&mut self) -> Result<()> {
        if self.sync_failed {
            bail!("failed to forward file changes to ty");
        }
        self.session.ensure_running()
    }

    /// Forward disk changes since the last request to ty and refresh changed open documents.
    ///
    /// # Errors
    ///
    /// Returns an error when the watcher lost events or ty can't be notified.
    pub(super) fn sync_workspace_changes(&mut self) -> Result<()> {
        // The changes are consumed either way, so a failed sync leaves ty out of date for good.
        self.forward_workspace_changes()
            .inspect_err(|_| self.sync_failed = true)
    }

    fn forward_workspace_changes(&mut self) -> Result<()> {
        let changes = self.watcher.take_changes()?;
        if changes.is_empty() {
            return Ok(());
        }
        // Open documents override disk contents in ty, so they need their own update.
        for (path, change) in &changes {
            if *change == FileChangeType::Deleted {
                // A deleted directory takes its open documents with it.
                let closed: Vec<PathBuf> = self
                    .documents
                    .keys()
                    .filter(|document| document.starts_with(path))
                    .cloned()
                    .collect();
                for document in closed {
                    self.documents.remove(&document);
                    self.session.close_file(&document)?;
                }
            } else if self.documents.contains_key(path) {
                self.ensure_file_synced(path)?;
            }
        }
        self.session.change_watched_files(&changes)
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
        let method = match target {
            GotoTarget::Definition => "textDocument/definition",
            GotoTarget::Declaration => "textDocument/declaration",
            GotoTarget::Type => "textDocument/typeDefinition",
        };
        let params = self.position_params(position)?;
        self.locations(method, params)
    }

    pub(super) fn reference_locations(
        &mut self,
        position: &ResolvedPosition,
        declaration: Declaration,
    ) -> Result<Vec<LocationRecord>> {
        let mut params = self.position_params(position)?;
        params["context"] = json!({ "includeDeclaration": declaration == Declaration::Include });
        self.locations("textDocument/references", params)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::lsp::model::{PositionRecord, RangeRecord, SymbolKind, WorkspaceSymbolRecord};

    use super::rank_workspace_symbols;

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
}
