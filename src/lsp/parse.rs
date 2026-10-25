use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde_json::Value;
use url::Url;

use crate::lsp::model::{
    DocumentSymbolNode, LocationRecord, PositionRecord, RangeRecord, RenameEditRecord,
    SymbolAtRecord, SymbolKind, WorkspaceSymbolRecord,
};

/// Zero-based LSP wire position, converted into a 1-based [`PositionRecord`].
#[derive(Deserialize)]
pub(crate) struct LspPosition {
    line: usize,
    character: usize,
}

impl From<LspPosition> for PositionRecord {
    fn from(position: LspPosition) -> Self {
        Self {
            line: position.line + 1,
            column: position.character + 1,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LspLocation {
    #[serde(rename_all = "camelCase")]
    Link {
        target_uri: String,
        target_selection_range: RangeRecord,
    },
    Location(LspUriRange),
}

#[derive(Deserialize)]
struct LspUriRange {
    uri: String,
    range: RangeRecord,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LspSymbolInformation {
    name: String,
    kind: SymbolKind,
    container_name: Option<String>,
    location: LspUriRange,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LspDocumentSymbol {
    Nested(DocumentSymbolNode),
    Flat(LspSymbolInformation),
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LspWorkspaceEdit {
    changes: Option<HashMap<String, Vec<LspTextEdit>>>,
    document_changes: Option<Vec<LspDocumentChange>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LspDocumentChange {
    #[serde(rename_all = "camelCase")]
    Edit {
        text_document: LspDocumentId,
        edits: Vec<LspTextEdit>,
    },
    Resource(IgnoredAny),
}

#[derive(Deserialize)]
struct LspDocumentId {
    uri: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LspTextEdit {
    range: RangeRecord,
    new_text: String,
}

/// Deserialize an LSP result, treating `null` as the default (empty) value.
fn parse_result<T: DeserializeOwned + Default>(value: Value, method: &str) -> Result<T> {
    Ok(serde_json::from_value::<Option<T>>(value)
        .with_context(|| format!("invalid {method} response"))?
        .unwrap_or_default())
}

pub(crate) fn parse_location_response(value: Value) -> Result<Vec<LocationRecord>> {
    let locations = match serde_json::from_value::<Option<OneOrMany<LspLocation>>>(value)
        .context("invalid location response")?
    {
        None => Vec::new(),
        Some(OneOrMany::One(location)) => vec![location],
        Some(OneOrMany::Many(locations)) => locations,
    };

    locations
        .into_iter()
        .map(|location| {
            let (uri, range) = match location {
                LspLocation::Link {
                    target_uri,
                    target_selection_range,
                } => (target_uri, target_selection_range),
                LspLocation::Location(LspUriRange { uri, range }) => (uri, range),
            };
            Ok(LocationRecord {
                file: file_uri_to_path(&uri)?,
                range,
                snippet: None,
            })
        })
        .collect()
}

pub(crate) fn parse_hover_contents(value: Value) -> Result<String> {
    if value.is_null() {
        return Ok(String::new());
    }

    let contents = value
        .get("contents")
        .context("hover response missing contents")?;
    Ok(stringify_hover_contents(contents))
}

fn stringify_hover_contents(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(stringify_hover_contents)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(map) => match (
            map.get("language").and_then(Value::as_str),
            map.get("value"),
        ) {
            // `MarkedString` with a language is a fenced code block.
            (Some(language), Some(body)) => {
                format!("```{language}\n{}\n```", stringify_hover_contents(body))
            }
            // `MarkupContent` already carries rendered markdown or plain text.
            (None, Some(body)) => stringify_hover_contents(body),
            _ => serde_json::to_string_pretty(value).unwrap_or_default(),
        },
        _ => value.to_string(),
    }
}

pub(crate) fn parse_document_symbols(value: Value) -> Result<Vec<DocumentSymbolNode>> {
    let symbols: Vec<LspDocumentSymbol> = parse_result(value, "textDocument/documentSymbol")?;

    Ok(symbols
        .into_iter()
        .map(|symbol| match symbol {
            LspDocumentSymbol::Nested(node) => node,
            LspDocumentSymbol::Flat(info) => DocumentSymbolNode {
                name: info.name,
                detail: None,
                kind: info.kind,
                range: info.location.range,
                selection_range: info.location.range,
                children: Vec::new(),
            },
        })
        .collect())
}

pub(crate) fn parse_workspace_symbols(value: Value) -> Result<Vec<WorkspaceSymbolRecord>> {
    let symbols: Vec<LspSymbolInformation> = parse_result(value, "workspace/symbol")?;

    symbols
        .into_iter()
        .map(|symbol| {
            Ok(WorkspaceSymbolRecord {
                name: symbol.name,
                kind: symbol.kind,
                container_name: symbol.container_name,
                file: file_uri_to_path(&symbol.location.uri)?,
                range: symbol.location.range,
                snippet: None,
            })
        })
        .collect()
}

pub(crate) fn parse_workspace_edit(value: Value) -> Result<Vec<RenameEditRecord>> {
    let edit: LspWorkspaceEdit = parse_result(value, "textDocument/rename")?;
    let document_changes = edit
        .document_changes
        .unwrap_or_default()
        .into_iter()
        .map(|change| match change {
            LspDocumentChange::Edit {
                text_document,
                edits,
            } => Ok((text_document.uri, edits)),
            LspDocumentChange::Resource(_) => {
                bail!("rename response contained an unsupported resource operation")
            }
        });

    let mut output = Vec::new();
    for change in edit
        .changes
        .unwrap_or_default()
        .into_iter()
        .map(Ok)
        .chain(document_changes)
    {
        let (uri, edits) = change?;
        let file = file_uri_to_path(&uri)?;
        output.extend(edits.into_iter().map(|edit| RenameEditRecord {
            file: file.clone(),
            range: edit.range,
            new_text: edit.new_text,
        }));
    }

    output.sort_by(|left, right| {
        (&left.file, left.range.start).cmp(&(&right.file, right.range.start))
    });
    Ok(output)
}

pub(crate) fn build_symbol_hierarchy(symbols: Vec<DocumentSymbolNode>) -> Vec<DocumentSymbolNode> {
    let mut roots = Vec::new();
    for symbol in symbols {
        insert_document_symbol(&mut roots, symbol);
    }
    roots
}

fn insert_document_symbol(nodes: &mut Vec<DocumentSymbolNode>, symbol: DocumentSymbolNode) {
    match nodes.iter_mut().rev().find(|existing| {
        existing.range.contains(symbol.range.start) && existing.range.contains(symbol.range.end)
    }) {
        Some(parent) => insert_document_symbol(&mut parent.children, symbol),
        None => nodes.push(symbol),
    }
}

pub(crate) fn prune_outline_depth(
    symbols: Vec<DocumentSymbolNode>,
    depth: usize,
) -> Vec<DocumentSymbolNode> {
    if depth == 0 {
        return Vec::new();
    }

    symbols
        .into_iter()
        .map(|symbol| DocumentSymbolNode {
            children: prune_outline_depth(symbol.children, depth - 1),
            ..symbol
        })
        .collect()
}

/// Extract the identifier at (or just before) a 1-based column of `line`.
pub(crate) fn extract_symbol_at(
    line: &str,
    column_number: usize,
) -> Result<Option<SymbolAtRecord>> {
    let characters = line.chars().collect::<Vec<_>>();

    if column_number == 0 || column_number > characters.len() + 1 {
        bail!(
            "column {column_number} is out of range for line of length {}",
            characters.len()
        );
    }

    if characters.is_empty() {
        return Ok(None);
    }

    let mut index = (column_number - 1).min(characters.len() - 1);
    if !is_symbol_char(characters[index]) && index > 0 && is_symbol_char(characters[index - 1]) {
        index -= 1;
    }

    if !is_symbol_char(characters[index]) {
        return Ok(None);
    }

    let mut start = index;
    while start > 0 && is_symbol_char(characters[start - 1]) {
        start -= 1;
    }

    let mut end = index + 1;
    while end < characters.len() && is_symbol_char(characters[end]) {
        end += 1;
    }

    Ok(Some(SymbolAtRecord {
        name: characters[start..end].iter().collect(),
        start_column: start + 1,
        end_column: end + 1,
        kind: None,
        detail: None,
    }))
}

pub(crate) fn find_document_symbol(
    symbols: &[DocumentSymbolNode],
    position: PositionRecord,
) -> Option<&DocumentSymbolNode> {
    let symbol = symbols
        .iter()
        .find(|symbol| symbol.selection_range.contains(position))?;
    find_document_symbol(&symbol.children, position).or(Some(symbol))
}

pub(crate) fn is_symbol_char(value: char) -> bool {
    value == '_' || value.is_alphanumeric()
}

fn file_uri_to_path(uri: &str) -> Result<PathBuf> {
    Url::parse(uri)
        .with_context(|| format!("failed to parse URI {uri}"))?
        .to_file_path()
        .map_err(|()| anyhow!("failed to convert URI {uri} to path"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{
        parse_document_symbols, parse_hover_contents, parse_location_response, parse_workspace_edit,
    };

    #[test]
    fn parse_location_link_uses_target_selection_range() {
        let value = json!({
            "targetUri": "file:///tmp/example.py",
            "targetRange": {
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 10}
            },
            "targetSelectionRange": {
                "start": {"line": 1, "character": 2},
                "end": {"line": 1, "character": 5}
            }
        });

        let parsed = parse_location_response(value).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].range.start.line, 2);
        assert_eq!(parsed[0].range.start.column, 3);
    }

    #[test]
    fn parse_location_array_and_null() {
        let value = json!([{
            "uri": "file:///tmp/example.py",
            "range": {
                "start": {"line": 4, "character": 0},
                "end": {"line": 4, "character": 3}
            }
        }]);

        let parsed = parse_location_response(value).unwrap();
        assert_eq!(parsed[0].file, Path::new("/tmp/example.py"));
        assert_eq!(parsed[0].range.end.column, 4);
        assert!(parse_location_response(json!(null)).unwrap().is_empty());
    }

    #[test]
    fn parses_nested_and_flat_document_symbols() {
        let range = json!({
            "start": {"line": 0, "character": 0},
            "end": {"line": 2, "character": 0}
        });
        let value = json!([
            {
                "name": "User",
                "kind": 5,
                "range": range,
                "selectionRange": range,
                "children": [{"name": "save", "kind": 6, "range": range, "selectionRange": range}]
            },
            {
                "name": "main",
                "kind": 12,
                "location": {"uri": "file:///tmp/example.py", "range": range}
            }
        ]);

        let parsed = parse_document_symbols(value).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].children[0].name, "save");
        assert_eq!(parsed[1].name, "main");
        assert_eq!(parsed[1].selection_range.end.line, 3);
    }

    #[test]
    fn hover_formats_marked_string_with_language() {
        let value = json!({"contents": [{"language": "python", "value": "def f(): ..."}, "doc"]});

        assert_eq!(
            parse_hover_contents(value).unwrap(),
            "```python\ndef f(): ...\n```\n\ndoc"
        );
    }

    #[test]
    fn parses_workspace_edit_changes() {
        let value = json!({
            "changes": {
                "file:///tmp/example.py": [
                    {
                        "range": {
                            "start": {"line": 1, "character": 2},
                            "end": {"line": 1, "character": 6}
                        },
                        "newText": "Renamed"
                    }
                ]
            }
        });

        let parsed = parse_workspace_edit(value).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].file, Path::new("/tmp/example.py"));
        assert_eq!(parsed[0].range.start.line, 2);
        assert_eq!(parsed[0].range.start.column, 3);
        assert_eq!(parsed[0].new_text, "Renamed");
    }

    #[test]
    fn parses_workspace_edit_document_changes() {
        let value = json!({
            "documentChanges": [
                {
                    "textDocument": {
                        "uri": "file:///tmp/example.py",
                        "version": 1
                    },
                    "edits": [
                        {
                            "range": {
                                "start": {"line": 3, "character": 4},
                                "end": {"line": 3, "character": 8}
                            },
                            "newText": "Renamed"
                        }
                    ]
                }
            ]
        });

        let parsed = parse_workspace_edit(value).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].range.start.line, 4);
        assert_eq!(parsed[0].new_text, "Renamed");
    }

    #[test]
    fn rejects_workspace_edit_resource_operations() {
        let value = json!({"documentChanges": [{"kind": "create", "uri": "file:///tmp/new.py"}]});

        assert!(
            parse_workspace_edit(value)
                .unwrap_err()
                .to_string()
                .contains("unsupported resource operation")
        );
    }
}
