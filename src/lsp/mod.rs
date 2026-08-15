pub(crate) mod model;
pub(crate) mod parse;
pub(crate) mod rename;

use std::env;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use url::Url;

use crate::workspace::ty_server_configuration;

pub(crate) struct LspSession {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl LspSession {
    pub(crate) fn start(binary: &Path, workspace_root: &Path) -> Result<Self> {
        let stderr = if env::var_os("LSPYX_DEBUG").is_some() {
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
        let response = self.request_internal(
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
        self.request_internal(method, params)
    }

    fn request_internal(&mut self, method: &str, params: Value) -> Result<Value> {
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
            let message = self.read_message()?;

            // Ignore notifications and only return the response that matches our request.
            if let Some(message_id) = message.get("id").and_then(Value::as_i64) {
                if message_id != id {
                    continue;
                }

                if let Some(error) = message.get("error") {
                    return Err(anyhow!("LSP request {method} failed: {error}"));
                }

                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
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
        let _ = self.request_internal("shutdown", json!(null));
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

        loop {
            let mut line = String::new();
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

pub(crate) fn read_line_text(path: &Path, line_number: usize) -> Result<String> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

    text.lines()
        .nth(line_number.saturating_sub(1))
        .map(ToString::to_string)
        .ok_or_else(|| {
            anyhow!(
                "line {} is out of range for {}",
                line_number,
                path.display()
            )
        })
}

pub(crate) fn column_to_utf16_offset(line_text: &str, column_number: usize) -> Result<usize> {
    if column_number == 0 {
        bail!("column must be 1-based");
    }

    let scalar_count = line_text.chars().count();
    if column_number > scalar_count + 1 {
        bail!(
            "column {} is out of range for line of length {}",
            column_number,
            scalar_count
        );
    }

    let prefix: String = line_text.chars().take(column_number - 1).collect();
    Ok(prefix.encode_utf16().count())
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
