use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::{GotoTarget, SymbolKindFilter};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DaemonWireResponse {
    pub(crate) ok: bool,
    pub(crate) payload: Option<Value>,
    pub(crate) text: Option<String>,
    pub(crate) error: Option<String>,
}

impl DaemonWireResponse {
    /// Build a successful response with rendered text and an optional structured payload.
    pub(crate) fn success(text: String, payload: Option<Value>) -> Self {
        Self {
            ok: true,
            payload,
            text: Some(text),
            error: None,
        }
    }

    /// Build a failed response carrying the full error chain.
    pub(crate) fn failure(error: &anyhow::Error) -> Self {
        Self {
            ok: false,
            payload: None,
            text: None,
            error: Some(format!("{error:#}")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "kebab-case")]
pub(crate) enum DaemonRequest {
    Ping,
    Shutdown,
    Goto {
        file: PathBuf,
        line: usize,
        column: usize,
        target: GotoTarget,
        limit: Option<usize>,
    },
    Usages {
        file: PathBuf,
        line: usize,
        column: usize,
        include_declaration: bool,
        limit: Option<usize>,
    },
    FindSymbol {
        query: String,
        kind: Option<SymbolKindFilter>,
        limit: Option<usize>,
    },
    Inspect {
        file: PathBuf,
        line: usize,
        column: usize,
    },
    /// Inspect a position and, when it names a symbol, add its definition and usages.
    Explore {
        file: PathBuf,
        line: usize,
        column: usize,
        limit: Option<usize>,
    },
    Outline {
        file: PathBuf,
        depth: Option<usize>,
        limit: Option<usize>,
    },
    Rename {
        file: PathBuf,
        line: usize,
        column: usize,
        new_name: String,
    },
}

pub(super) fn send_request(
    socket_path: &Path,
    request: &DaemonRequest,
) -> Result<Option<DaemonWireResponse>> {
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
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
        .downcast_ref::<std::io::Error>()
        .is_some_and(|source| source.kind() == std::io::ErrorKind::UnexpectedEof)
}
