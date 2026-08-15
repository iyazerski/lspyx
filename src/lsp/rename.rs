use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use similar::TextDiff;

use crate::lsp::model::{PositionRecord, RenameEditRecord, display_path};

/// Build a complete unified diff from validated semantic rename edits.
pub(crate) fn build_rename_diff(
    workspace_root: &Path,
    old_name: &str,
    edits: &[RenameEditRecord],
) -> Result<String> {
    let mut edits_by_file = BTreeMap::<PathBuf, Vec<&RenameEditRecord>>::new();
    for edit in edits {
        edits_by_file
            .entry(edit.file.clone())
            .or_default()
            .push(edit);
    }

    let mut patches = Vec::new();
    for (file, file_edits) in edits_by_file {
        let original = fs::read_to_string(&file)
            .with_context(|| format!("failed to read rename target {}", file.display()))?;
        let updated = apply_rename_edits(&original, old_name, &file, file_edits)?;
        let relative = display_path(workspace_root, &file);
        let old_header = format!("a/{relative}");
        let new_header = format!("b/{relative}");
        let patch = TextDiff::from_lines(&original, &updated)
            .unified_diff()
            .context_radius(3)
            .header(&old_header, &new_header)
            .to_string();
        patches.push(patch);
    }

    Ok(patches.join(""))
}

fn apply_rename_edits(
    original: &str,
    old_name: &str,
    file: &Path,
    edits: Vec<&RenameEditRecord>,
) -> Result<String> {
    let mut ranges = edits
        .into_iter()
        .map(|edit| {
            let start = position_to_byte_offset(original, &edit.range.start)?;
            let end = position_to_byte_offset(original, &edit.range.end)?;
            if start > end {
                bail!("rename edit has an inverted range in {}", file.display());
            }
            let current = &original[start..end];
            if current != old_name {
                bail!(
                    "rename edit is stale in {}:{}:{}: expected {:?}, found {:?}",
                    file.display(),
                    edit.range.start.line,
                    edit.range.start.column,
                    old_name,
                    current,
                );
            }
            Ok((start, end, edit.new_text.as_str()))
        })
        .collect::<Result<Vec<_>>>()?;
    ranges.sort_by(|left, right| right.0.cmp(&left.0));

    let mut updated = original.to_string();
    let mut previous_start = original.len();
    for (start, end, replacement) in ranges {
        if end > previous_start {
            bail!("rename edits overlap in {}", file.display());
        }
        updated.replace_range(start..end, replacement);
        previous_start = start;
    }
    Ok(updated)
}

fn position_to_byte_offset(text: &str, position: &PositionRecord) -> Result<usize> {
    if position.line == 0 || position.column == 0 {
        bail!("rename edit positions must be 1-based");
    }

    let mut line_start = 0;
    for _ in 1..position.line {
        let relative_end = text[line_start..]
            .find('\n')
            .context("rename edit line is out of range")?;
        line_start += relative_end + 1;
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |relative| line_start + relative);
    let mut line = &text[line_start..line_end];
    if let Some(without_carriage_return) = line.strip_suffix('\r') {
        line = without_carriage_return;
    }

    let target_utf16 = position.column - 1;
    let mut utf16_offset = 0;
    for (byte_offset, character) in line.char_indices() {
        if utf16_offset == target_utf16 {
            return Ok(line_start + byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset > target_utf16 {
            bail!("rename edit column splits a UTF-16 character");
        }
    }
    if utf16_offset == target_utf16 {
        return Ok(line_start + line.len());
    }
    bail!("rename edit column is out of range")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::lsp::model::{PositionRecord, RangeRecord, RenameEditRecord};

    use super::{apply_rename_edits, position_to_byte_offset};

    #[test]
    fn applies_multiple_edits_from_the_end() {
        let original = "value = Chat(Chat.id)\n";
        let edits = [
            edit(1, 9, 13, "Conversation"),
            edit(1, 14, 18, "Conversation"),
        ];

        let updated = apply_rename_edits(
            original,
            "Chat",
            Path::new("example.py"),
            edits.iter().collect(),
        )
        .unwrap();

        assert_eq!(updated, "value = Conversation(Conversation.id)\n");
    }

    #[test]
    fn converts_utf16_columns_to_byte_offsets() {
        let text = "x = '😀'; Chat\n";
        let offset = position_to_byte_offset(
            text,
            &PositionRecord {
                line: 1,
                column: 11,
            },
        )
        .unwrap();

        assert_eq!(&text[offset..offset + 4], "Chat");
    }

    #[test]
    fn rejects_stale_edits() {
        let edits = [edit(1, 1, 5, "Conversation")];

        let error = apply_rename_edits(
            "User = 1\n",
            "Chat",
            Path::new("example.py"),
            edits.iter().collect(),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("expected \"Chat\", found \"User\"")
        );
    }

    fn edit(line: usize, start: usize, end: usize, new_text: &str) -> RenameEditRecord {
        RenameEditRecord {
            file: "example.py".into(),
            range: RangeRecord {
                start: PositionRecord {
                    line,
                    column: start,
                },
                end: PositionRecord { line, column: end },
            },
            new_text: new_text.to_string(),
        }
    }
}
