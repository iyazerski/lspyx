//! A 1-based position in a file, shared by CLI, MCP, and daemon requests.

use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Error, Result, bail};
use serde::{Deserialize, Serialize};

/// A 1-based line and column in `file`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FilePosition {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

impl FilePosition {
    /// Build a position from user input.
    ///
    /// # Errors
    ///
    /// Returns an error when `line` or `column` is zero.
    pub(crate) fn new(file: PathBuf, line: usize, column: usize) -> Result<Self> {
        if line == 0 {
            bail!("line must be a 1-based value");
        }
        if column == 0 {
            bail!("column must be a 1-based value");
        }
        Ok(Self { file, line, column })
    }
}

impl FromStr for FilePosition {
    type Err = Error;

    /// Parse `file:line:column`, splitting from the right so paths may contain colons.
    fn from_str(input: &str) -> Result<Self> {
        let format_error = || format!("expected FILE:LINE:COLUMN format, got: {input}");
        let parts: Vec<&str> = input.rsplitn(3, ':').collect();
        let [column, line, file] = parts.as_slice() else {
            bail!(format_error());
        };
        if file.is_empty() {
            bail!(format_error());
        }

        let line: usize = line.parse().with_context(format_error)?;
        let column: usize = column.parse().with_context(format_error)?;
        Self::new(PathBuf::from(file), line, column)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::FilePosition;

    #[test]
    fn parses_paths_that_contain_colons() {
        let position: FilePosition = "C:/src/app.py:42:17".parse().unwrap();

        assert_eq!(position.file, Path::new("C:/src/app.py"));
        assert_eq!((position.line, position.column), (42, 17));
    }

    #[test]
    fn rejects_malformed_and_zero_positions() {
        for (input, expected) in [
            (
                "app.py:1",
                "expected FILE:LINE:COLUMN format, got: app.py:1",
            ),
            (":1:1", "expected FILE:LINE:COLUMN format, got: :1:1"),
            (
                "app.py:x:1",
                "expected FILE:LINE:COLUMN format, got: app.py:x:1",
            ),
            ("app.py:0:1", "line must be a 1-based value"),
            ("app.py:1:0", "column must be a 1-based value"),
        ] {
            let error = input.parse::<FilePosition>().unwrap_err();
            assert_eq!(error.to_string(), expected, "input {input:?}");
        }
    }
}
