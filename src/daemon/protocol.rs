use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::{GotoTarget, SymbolKindFilter};
use crate::file_position::FilePosition;
use crate::lsp::model::OutlineDepth;

/// A successful daemon reply: rendered text plus an optional structured payload.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DaemonReply {
    pub(crate) text: String,
    pub(crate) payload: Option<Value>,
}

impl DaemonReply {
    /// Build a reply that carries only rendered text.
    pub(crate) fn text(text: String) -> Self {
        Self {
            text,
            payload: None,
        }
    }

    /// Build a reply with rendered text and a structured payload.
    pub(crate) fn with_payload(text: String, payload: Value) -> Self {
        Self {
            text,
            payload: Some(payload),
        }
    }
}

/// A daemon response on the wire; failures carry the request's full error chain.
pub(super) type DaemonResponse = Result<DaemonReply, String>;

/// Whether a usages search includes the declaration site.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Declaration {
    Include,
    Exclude,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub(crate) enum DaemonRequest {
    Ping,
    Shutdown,
    Goto {
        position: FilePosition,
        target: GotoTarget,
        limit: Option<usize>,
    },
    Usages {
        position: FilePosition,
        declaration: Declaration,
        limit: Option<usize>,
    },
    FindSymbol {
        query: String,
        kind: Option<SymbolKindFilter>,
        limit: Option<usize>,
    },
    Inspect {
        position: FilePosition,
    },
    /// Inspect a position and, when it names a symbol, add its definition and usages.
    Explore {
        position: FilePosition,
        limit: Option<usize>,
    },
    Outline {
        file: PathBuf,
        depth: OutlineDepth,
        limit: Option<usize>,
    },
    Rename {
        position: FilePosition,
        new_name: String,
    },
}

/// Send `request` to the daemon at `socket_path`.
///
/// Returns `None` when no daemon is listening or it hung up before replying; a stale
/// socket file is removed in that case.
///
/// # Errors
///
/// Returns an error when the connection or the frame exchange fails unexpectedly.
pub(super) fn send_request(
    socket_path: &Path,
    request: &DaemonRequest,
) -> Result<Option<DaemonResponse>> {
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            let _ = fs::remove_file(socket_path);
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to connect to daemon {}", socket_path.display()));
        }
    };

    write_frame(&mut stream, request)?;
    stream.shutdown(Shutdown::Write)?;

    let response_body = match read_frame(&mut stream) {
        Ok(body) => body,
        Err(error) if is_unexpected_eof(&error) => return Ok(None),
        Err(error) => return Err(error),
    };

    Ok(Some(serde_json::from_slice(&response_body)?))
}

pub(super) fn read_request(stream: &mut UnixStream) -> Result<DaemonRequest> {
    let body = read_frame(stream)?;
    serde_json::from_slice(&body).context("failed to parse daemon request")
}

fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut length_bytes = [0_u8; 8];
    stream.read_exact(&mut length_bytes)?;

    let length = u64::from_be_bytes(length_bytes);
    let length = usize::try_from(length).context("daemon frame length exceeded usize")?;
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    Ok(body)
}

/// Write `value` as JSON behind a big-endian u64 length prefix in a single write.
pub(super) fn write_frame(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    // Serialize behind a placeholder prefix so the body is never copied.
    let mut frame = vec![0_u8; 8];
    serde_json::to_writer(&mut frame, value)?;
    let length = u64::try_from(frame.len() - 8).context("daemon frame length exceeded u64")?;
    frame[..8].copy_from_slice(&length.to_be_bytes());
    stream.write_all(&frame)?;
    Ok(())
}

fn is_unexpected_eof(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|source| source.kind() == io::ErrorKind::UnexpectedEof)
}
