use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use url::Url;

use crate::lsp::read_line_text;
use crate::model::{
    DocumentSymbolNode, LocationRecord, PositionRecord, RangeRecord, SymbolAtRecord,
    WorkspaceSymbolRecord,
};

pub(crate) fn parse_location_response(value: Value) -> Result<Vec<LocationRecord>> {
    if value.is_null() {
        return Ok(Vec::new());
    }

    let entries = match value {
        Value::Array(items) => items,
        other => vec![other],
    };

    let mut output = Vec::new();

    for entry in entries {
        let (uri, range) = if entry.get("targetUri").is_some() {
            (
                entry
                    .get("targetUri")
                    .and_then(Value::as_str)
                    .context("missing targetUri")?,
                entry
                    .get("targetSelectionRange")
                    .or_else(|| entry.get("targetRange"))
                    .context("missing target range")?,
            )
        } else {
            (
                entry
                    .get("uri")
                    .and_then(Value::as_str)
                    .context("missing uri")?,
                entry.get("range").context("missing range")?,
            )
        };

        output.push(LocationRecord {
            file: file_uri_to_path(uri)?,
            range: parse_range(range)?,
            snippet: None,
        });
    }

    Ok(output)
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
        Value::Object(map) => {
            if let Some(value) = map.get("value") {
                stringify_hover_contents(value)
            } else if let Some(language) = map.get("language").and_then(Value::as_str) {
                let body = map
                    .get("value")
                    .map(stringify_hover_contents)
                    .unwrap_or_default();
                format!("```{language}\n{body}\n```")
            } else {
                serde_json::to_string_pretty(value).unwrap_or_default()
            }
        }
        _ => value.to_string(),
    }
}

pub(crate) fn parse_document_symbols(value: Value) -> Result<Vec<DocumentSymbolNode>> {
    if value.is_null() {
        return Ok(Vec::new());
    }

    let items = value
        .as_array()
        .context("documentSymbol response was not an array")?;
    let mut output = Vec::new();

    for item in items {
        if item.get("selectionRange").is_some() {
            output.push(parse_document_symbol_node(item)?);
        } else {
            output.push(symbol_information_to_document_symbol(item)?);
        }
    }

    Ok(output)
}

fn parse_document_symbol_node(value: &Value) -> Result<DocumentSymbolNode> {
    let children = value
        .get("children")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(parse_document_symbol_node)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(DocumentSymbolNode {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .context("document symbol missing name")?
            .to_string(),
        detail: value
            .get("detail")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        kind: value
            .get("kind")
            .and_then(Value::as_u64)
            .context("document symbol missing kind")?,
        range: parse_range(
            value
                .get("range")
                .context("document symbol missing range")?,
        )?,
        selection_range: parse_range(
            value
                .get("selectionRange")
                .context("document symbol missing selectionRange")?,
        )?,
        children,
    })
}

fn symbol_information_to_document_symbol(value: &Value) -> Result<DocumentSymbolNode> {
    let location = value
        .get("location")
        .context("symbol information missing location")?;
    let range = parse_range(location.get("range").context("symbol info missing range")?)?;

    Ok(DocumentSymbolNode {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .context("symbol information missing name")?
            .to_string(),
        detail: None,
        kind: value
            .get("kind")
            .and_then(Value::as_u64)
            .context("symbol information missing kind")?,
        range: range.clone(),
        selection_range: range,
        children: Vec::new(),
    })
}

pub(crate) fn parse_workspace_symbols(value: Value) -> Result<Vec<WorkspaceSymbolRecord>> {
    if value.is_null() {
        return Ok(Vec::new());
    }

    let items = value
        .as_array()
        .context("workspace/symbol response was not an array")?;
    let mut output = Vec::new();

    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .context("workspace symbol missing name")?
            .to_string();
        let kind = item
            .get("kind")
            .and_then(Value::as_u64)
            .context("workspace symbol missing kind")?;
        let container_name = item
            .get("containerName")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        let location = item
            .get("location")
            .context("workspace symbol missing location")?;
        let uri = location
            .get("uri")
            .and_then(Value::as_str)
            .context("workspace symbol location missing uri")?;
        let range = parse_range(
            location
                .get("range")
                .context("workspace symbol location missing range")?,
        )?;

        output.push(WorkspaceSymbolRecord {
            name,
            kind,
            container_name,
            file: file_uri_to_path(uri)?,
            range,
            snippet: None,
        });
    }

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
    for existing in nodes.iter_mut().rev() {
        if range_contains(&existing.range, &symbol.range) {
            insert_document_symbol(&mut existing.children, symbol);
            return;
        }
    }

    nodes.push(symbol);
}

fn range_contains(parent: &RangeRecord, child: &RangeRecord) -> bool {
    let starts_before =
        (parent.start.line, parent.start.column) <= (child.start.line, child.start.column);
    let ends_after = (parent.end.line, parent.end.column) >= (child.end.line, child.end.column);
    starts_before && ends_after
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
            children: prune_outline_depth(symbol.children, depth.saturating_sub(1)),
            ..symbol
        })
        .collect()
}

pub(crate) fn extract_symbol_at(
    path: &Path,
    line_number: usize,
    column_number: usize,
) -> Result<Option<SymbolAtRecord>> {
    let line = read_line_text(path, line_number)?;
    let characters = line.chars().collect::<Vec<_>>();

    if column_number == 0 || column_number > characters.len() + 1 {
        bail!(
            "column {} is out of range for line of length {}",
            column_number,
            characters.len()
        );
    }

    if characters.is_empty() {
        return Ok(None);
    }

    let mut index = column_number.saturating_sub(1);
    if index >= characters.len() {
        index = characters.len().saturating_sub(1);
    }

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
    }))
}

fn is_symbol_char(value: char) -> bool {
    value == '_' || value.is_alphanumeric()
}

fn parse_range(value: &Value) -> Result<RangeRecord> {
    let start = value.get("start").context("range missing start")?;
    let end = value.get("end").context("range missing end")?;

    Ok(RangeRecord {
        start: parse_position(start)?,
        end: parse_position(end)?,
    })
}

fn parse_position(value: &Value) -> Result<PositionRecord> {
    Ok(PositionRecord {
        line: value
            .get("line")
            .and_then(Value::as_u64)
            .context("position missing line")? as usize
            + 1,
        column: value
            .get("character")
            .and_then(Value::as_u64)
            .context("position missing character")? as usize
            + 1,
    })
}

fn file_uri_to_path(uri: &str) -> Result<PathBuf> {
    Url::parse(uri)
        .with_context(|| format!("failed to parse URI {uri}"))?
        .to_file_path()
        .map_err(|()| anyhow!("failed to convert URI {uri} to path"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::parse_location_response;

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
}
