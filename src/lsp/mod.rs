pub(crate) mod model;
pub(crate) mod parse;
pub(crate) mod rename;

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use url::Url;

use crate::daemon::debug_enabled;
use crate::workspace::ty_server_configuration;

pub(crate) struct LspSession {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl LspSession {
    pub(crate) fn start(binary: &Path, workspace_root: &Path) -> Result<Self> {
        let stderr = if debug_enabled() {
            Stdio::inherit()
        } else {
            Stdio::null()
        };

        let mut child = Command::new(binary)
            .arg("server")
            .current_dir(workspace_root)
            .env("RUST_LOG", "error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .with_context(|| format!("failed to start {}", binary.display()))?;

        let child_stdin = child.stdin.take().context("missing ty stdin")?;
        let child_stdout = child.stdout.take().context("missing ty stdout")?;
        let mut session = Self {
            child,
            stdin: BufWriter::new(child_stdin),
            stdout: BufReader::new(child_stdout),
            next_id: 1,
        };

        session.initialize(workspace_root)?;
        Ok(session)
    }

    fn initialize(&mut self, workspace_root: &Path) -> Result<()> {
        let workspace_uri = path_to_file_uri(workspace_root)?;
        let ty_configuration = ty_server_configuration(workspace_root)?;
        let response = self.request(
            "initialize",
            json!({
                "processId": null,
                "clientInfo": {
                    "name": "lspyx",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "rootUri": workspace_uri,
                "capabilities": {},
                "initializationOptions": {
                    "settings": {
                        "ty": {
                            "configuration": ty_configuration,
                        }
                    }
                },
                "workspaceFolders": [
                    {
                        "uri": workspace_uri,
                        "name": workspace_root
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or("workspace"),
                    }
                ],
                "trace": "off",
            }),
        )?;

        if response.is_null() {
            bail!("initialize returned null");
        }

        self.notify("initialized", json!({}))?;
        self.notify(
            "workspace/didChangeConfiguration",
            json!({
                "settings": {
                    "ty": {
                        "configuration": ty_configuration,
                    }
                }
            }),
        )?;
        Ok(())
    }

    pub(crate) fn open_file_with_text(
        &mut self,
        file: &Path,
        version: i32,
        text: &str,
    ) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": path_to_file_uri(file)?,
                    "languageId": "python",
                    "version": version,
                    "text": text,
                }
            }),
        )
    }

    pub(crate) fn change_file(&mut self, file: &Path, version: i32, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {
                    "uri": path_to_file_uri(file)?,
                    "version": version,
                },
                "contentChanges": [
                    {
                        "text": text,
                    }
                ],
            }),
        )
    }

    pub(crate) fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_message(&payload)?;

        loop {
            let mut message = self.read_message()?;

            // Skip notifications and server-to-client requests, which carry a `method`
            // and may reuse ids from their own sequence.
            if message.get("method").is_some()
                || message.get("id").and_then(Value::as_i64) != Some(id)
            {
                continue;
            }

            if let Some(error) = message.get("error") {
                return Err(anyhow!("LSP request {method} failed: {error}"));
            }

            return Ok(message
                .get_mut("result")
                .map(Value::take)
                .unwrap_or(Value::Null));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_message(&payload)
    }

    pub(crate) fn shutdown(&mut self) -> Result<()> {
        let _ = self.request("shutdown", json!(null));
        let _ = self.notify("exit", json!(null));
        let _ = self.child.wait();
        Ok(())
    }

    fn write_message(&mut self, value: &Value) -> Result<()> {
        let body = serde_json::to_vec(value)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<Value> {
        let mut content_length = None;
        let mut line = String::new();

        loop {
            line.clear();
            let read = self.stdout.read_line(&mut line)?;
            if read == 0 {
                bail!("LSP server closed stdout unexpectedly");
            }

            if line == "\r\n" {
                break;
            }

            let trimmed = line.trim();
            if let Some(length) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(length.trim().parse::<usize>()?);
            }
        }

        let length = content_length.context("missing Content-Length header from LSP server")?;
        let mut buffer = vec![0_u8; length];
        self.stdout.read_exact(&mut buffer)?;
        Ok(serde_json::from_slice(&buffer)?)
    }
}

pub(crate) fn path_to_file_uri(path: &Path) -> Result<String> {
    Url::from_file_path(path)
        .map_err(|()| anyhow!("failed to convert {} to file URI", path.display()))
        .map(|value| value.to_string())
}

/// Read trimmed source lines for snippets, loading each file at most once.
#[derive(Default)]
pub(crate) struct SnippetReader {
    files: HashMap<PathBuf, Option<String>>,
}

impl SnippetReader {
    /// Return the trimmed 1-based `line` of `path`, or `None` when it cannot be read.
    pub(crate) fn snippet(&mut self, path: &Path, line: usize) -> Option<String> {
        let text = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| fs::read_to_string(path).ok());
        nth_line(text.as_deref()?, line).map(|value| value.trim().to_string())
    }
}

/// Return the 1-based `line_number` of `text`.
pub(crate) fn nth_line(text: &str, line_number: usize) -> Option<&str> {
    text.lines().nth(line_number.saturating_sub(1))
}

pub(crate) fn column_to_utf16_offset(line_text: &str, column_number: usize) -> Result<usize> {
    if column_number == 0 {
        bail!("column must be 1-based");
    }

    let mut characters = 0;
    let mut offset = 0;
    for character in line_text.chars().take(column_number - 1) {
        characters += 1;
        offset += character.len_utf16();
    }
    if characters < column_number - 1 {
        bail!(
            "column {} is out of range for line of length {}",
            column_number,
            characters
        );
    }

    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::column_to_utf16_offset;

    #[test]
    fn utf16_offset_counts_surrogate_pairs() {
        let line = "a😀b";
        assert_eq!(column_to_utf16_offset(line, 1).unwrap(), 0);
        assert_eq!(column_to_utf16_offset(line, 2).unwrap(), 1);
        assert_eq!(column_to_utf16_offset(line, 3).unwrap(), 3);
        assert_eq!(column_to_utf16_offset(line, 4).unwrap(), 4);
    }
}
