use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use crate::lsp::model::{
    DocumentSymbolNode, LocationRecord, RangeRecord, ResolvedPosition, WorkspaceSymbolRecord,
};
use crate::lsp::parse::{
    apply_document_symbol_metadata, extract_symbol_at, find_document_symbol,
    parse_document_symbols, parse_hover_contents, parse_location_response, parse_workspace_edit,
    parse_workspace_symbols,
};
use crate::lsp::{LspSession, column_to_utf16_offset, path_to_file_uri, read_line_text};
use crate::workspace::canonicalize_path;

pub(super) struct PersistentAdapter {
    session: LspSession,
    documents: HashMap<PathBuf, OpenDocument>,
}

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

    pub(super) fn shutdown(&mut self) -> Result<()> {
        self.session.shutdown()
    }

    pub(super) fn resolve_position(
        &mut self,
        file: &Path,
        line: usize,
        requested_column: usize,
    ) -> Result<ResolvedPosition> {
        self.ensure_file_synced(file)?;

        let source_line = read_line_text(file, line)
            .ok()
            .map(|value| value.trim().to_string());
        let symbol = extract_symbol_at(file, line, requested_column)?;
        let resolved_column = symbol.as_ref().map(|value| value.start_column);
        let symbol = if let Some(symbol) = symbol {
            let document_symbols = self.document_symbols(file)?;
            let document_symbol =
                find_document_symbol(document_symbols.as_slice(), line, symbol.start_column)
                    .filter(|value| value.name == symbol.name);
            Some(apply_document_symbol_metadata(symbol, document_symbol))
        } else {
            None
        };

        Ok(ResolvedPosition {
            file: file.to_path_buf(),
            line,
            requested_column,
            resolved_column,
            source_line,
            symbol,
        })
    }

    pub(super) fn definition_locations(
        &mut self,
        file: &Path,
        line: usize,
        column: usize,
    ) -> Result<Vec<LocationRecord>> {
        let locations =
            self.request_locations("textDocument/definition", file, line, column, false)?;
        self.resolve_imported_definition(file, line, column, locations)
    }

    pub(super) fn reference_locations(
        &mut self,
        workspace_root: &Path,
        file: &Path,
        line: usize,
        column: usize,
        include_declaration: bool,
    ) -> Result<Vec<LocationRecord>> {
        let locations = self.request_locations(
            "textDocument/references",
            file,
            line,
            column,
            include_declaration,
        )?;
        let unique_files = locations
            .iter()
            .map(|location| location.file.clone())
            .collect::<HashSet<_>>();
        if unique_files.len() > 1 {
            return Ok(locations);
        }

        let Some(symbol) = extract_symbol_at(file, line, column)? else {
            return Ok(locations);
        };
        let Some(canonical_symbol) = self.unique_workspace_symbol(symbol.name.as_str())? else {
            return Ok(locations);
        };

        let lexical_locations =
            collect_symbol_occurrences(workspace_root, canonical_symbol.name.as_str())?;
        Ok(merge_locations(locations, lexical_locations))
    }

    pub(super) fn request_locations(
        &mut self,
        method: &str,
        file: &Path,
        line: usize,
        column: usize,
        include_declaration: bool,
    ) -> Result<Vec<LocationRecord>> {
        self.ensure_file_synced(file)?;
        let utf16_character = self.utf16_offset(file, line, column)?;

        let params = if method == "textDocument/references" {
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                },
                "context": {
                    "includeDeclaration": include_declaration,
                }
            })
        } else {
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                }
            })
        };

        let response = self.session.request(method, params)?;
        let locations = parse_location_response(response)?;
        self.enrich_locations(locations)
    }

    fn resolve_imported_definition(
        &mut self,
        file: &Path,
        line: usize,
        column: usize,
        locations: Vec<LocationRecord>,
    ) -> Result<Vec<LocationRecord>> {
        let Some(symbol) = extract_symbol_at(file, line, column)? else {
            return Ok(locations);
        };
        let Some(unique_symbol) = self.unique_workspace_symbol(symbol.name.as_str())? else {
            return Ok(locations);
        };

        let redirected = locations
            .iter()
            .any(|location| is_import_location(location, symbol.name.as_str()));
        if !redirected {
            return Ok(locations);
        }

        Ok(vec![location_from_workspace_symbol(&unique_symbol)])
    }

    pub(super) fn hover(&mut self, file: &Path, line: usize, column: usize) -> Result<String> {
        self.ensure_file_synced(file)?;
        let utf16_character = self.utf16_offset(file, line, column)?;
        let response = self.session.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                }
            }),
        )?;
        parse_hover_contents(response)
    }

    pub(super) fn document_symbols(&mut self, file: &Path) -> Result<Vec<DocumentSymbolNode>> {
        self.ensure_file_synced(file)?;
        let response = self.session.request(
            "textDocument/documentSymbol",
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
            }),
        )?;
        parse_document_symbols(response)
    }

    pub(super) fn workspace_symbol(&mut self, query: &str) -> Result<Vec<WorkspaceSymbolRecord>> {
        let response = self
            .session
            .request("workspace/symbol", json!({ "query": query }))?;
        parse_workspace_symbols(response)
    }

    pub(super) fn rename(
        &mut self,
        file: &Path,
        line: usize,
        column: usize,
        new_name: &str,
    ) -> Result<Vec<crate::lsp::model::RenameEditRecord>> {
        self.ensure_file_synced(file)?;
        let utf16_character = self.utf16_offset(file, line, column)?;
        let response = self.session.request(
            "textDocument/rename",
            json!({
                "textDocument": { "uri": path_to_file_uri(file)? },
                "position": {
                    "line": line - 1,
                    "character": utf16_character,
                },
                "newName": new_name,
            }),
        )?;
        parse_workspace_edit(response)
    }

    fn unique_workspace_symbol(&mut self, query: &str) -> Result<Option<WorkspaceSymbolRecord>> {
        let symbols = self.workspace_symbol(query)?;
        let exact_case_sensitive = symbols
            .iter()
            .filter(|symbol| symbol.name == query)
            .cloned()
            .collect::<Vec<_>>();
        if exact_case_sensitive.len() == 1 {
            return Ok(exact_case_sensitive.into_iter().next());
        }

        let exact_case_insensitive = symbols
            .iter()
            .filter(|symbol| symbol.name.eq_ignore_ascii_case(query))
            .cloned()
            .collect::<Vec<_>>();
        if exact_case_insensitive.len() == 1 {
            return Ok(exact_case_insensitive.into_iter().next());
        }

        Ok(None)
    }

    fn ensure_file_synced(&mut self, file: &Path) -> Result<()> {
        let canonical = canonicalize_path(file)?;
        let text = fs::read_to_string(&canonical)
            .with_context(|| format!("failed to read {}", canonical.display()))?;

        match self.documents.get_mut(&canonical) {
            Some(document) if document.text != text => {
                document.version += 1;
                self.session
                    .change_file(&canonical, document.version, &text)?;
                document.text = text;
            }
            Some(_) => {}
            None => {
                self.session.open_file_with_text(&canonical, 1, &text)?;
                self.documents
                    .insert(canonical, OpenDocument { text, version: 1 });
            }
        }

        Ok(())
    }

    fn utf16_offset(&self, file: &Path, line: usize, column: usize) -> Result<usize> {
        let line_text = read_line_text(file, line)?;
        column_to_utf16_offset(&line_text, column)
    }

    fn enrich_locations(&self, locations: Vec<LocationRecord>) -> Result<Vec<LocationRecord>> {
        let mut enriched = Vec::with_capacity(locations.len());

        for mut location in locations {
            let snippet = read_line_text(&location.file, location.range.start.line).ok();
            location.snippet = snippet.map(|value| value.trim().to_string());
            enriched.push(location);
        }

        Ok(enriched)
    }
}

fn is_import_location(location: &LocationRecord, symbol_name: &str) -> bool {
    let Some(snippet) = location.snippet.as_deref() else {
        return false;
    };
    let trimmed = snippet.trim_start();

    (trimmed.starts_with("from ") || trimmed.starts_with("import "))
        && trimmed.contains(symbol_name)
}

fn location_from_workspace_symbol(symbol: &WorkspaceSymbolRecord) -> LocationRecord {
    LocationRecord {
        file: symbol.file.clone(),
        range: symbol.range.clone(),
        snippet: read_line_text(&symbol.file, symbol.range.start.line)
            .ok()
            .map(|value| value.trim().to_string()),
    }
}

fn collect_symbol_occurrences(
    workspace_root: &Path,
    symbol_name: &str,
) -> Result<Vec<LocationRecord>> {
    let mut locations = Vec::new();
    collect_symbol_occurrences_recursive(
        workspace_root,
        workspace_root,
        symbol_name,
        &mut locations,
    )?;
    Ok(locations)
}

fn collect_symbol_occurrences_recursive(
    workspace_root: &Path,
    current: &Path,
    symbol_name: &str,
    locations: &mut Vec<LocationRecord>,
) -> Result<()> {
    if is_ignored_directory(current) {
        return Ok(());
    }

    for entry in
        fs::read_dir(current).with_context(|| format!("failed to read {}", current.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            if !is_ignored_directory(&path) {
                collect_symbol_occurrences_recursive(
                    workspace_root,
                    &path,
                    symbol_name,
                    locations,
                )?;
            }
            continue;
        }

        if !is_python_file(&path) {
            continue;
        }

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        for (line_index, line) in text.lines().enumerate() {
            for column in symbol_columns(line, symbol_name) {
                locations.push(LocationRecord {
                    file: path.clone(),
                    range: RangeRecord {
                        start: crate::lsp::model::PositionRecord {
                            line: line_index + 1,
                            column,
                        },
                        end: crate::lsp::model::PositionRecord {
                            line: line_index + 1,
                            column: column + symbol_name.chars().count(),
                        },
                    },
                    snippet: Some(line.trim().to_string()),
                });
            }
        }
    }

    let _ = workspace_root;
    Ok(())
}

fn symbol_columns(line: &str, symbol_name: &str) -> Vec<usize> {
    let mut columns = Vec::new();

    for (byte_index, _) in line.match_indices(symbol_name) {
        let before = line[..byte_index].chars().next_back();
        let after = line[byte_index + symbol_name.len()..].chars().next();

        if before.is_some_and(is_symbol_char_for_search)
            || after.is_some_and(is_symbol_char_for_search)
        {
            continue;
        }

        columns.push(line[..byte_index].chars().count() + 1);
    }

    columns
}

fn is_symbol_char_for_search(value: char) -> bool {
    value == '_' || value.is_alphanumeric()
}

fn is_python_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("py") | Some("pyi")
    )
}

fn is_ignored_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|value| value.to_str()),
        Some(".git")
            | Some(".hg")
            | Some(".mypy_cache")
            | Some(".pytest_cache")
            | Some(".ruff_cache")
            | Some(".tox")
            | Some(".venv")
            | Some("__pycache__")
            | Some("node_modules")
            | Some("target")
    )
}

fn merge_locations(
    mut primary: Vec<LocationRecord>,
    additional: Vec<LocationRecord>,
) -> Vec<LocationRecord> {
    let mut seen = primary
        .iter()
        .map(|location| {
            (
                location.file.clone(),
                location.range.start.line,
                location.range.start.column,
            )
        })
        .collect::<HashSet<_>>();

    for location in additional {
        let key = (
            location.file.clone(),
            location.range.start.line,
            location.range.start.column,
        );
        if seen.insert(key) {
            primary.push(location);
        }
    }

    primary
}
