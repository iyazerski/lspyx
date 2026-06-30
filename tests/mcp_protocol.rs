use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

struct McpServer {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<String>,
    reader: Option<JoinHandle<()>>,
}

impl McpServer {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lspyx"))
            .arg("mcp")
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start lspyx mcp server");
        let stdin = child.stdin.take().expect("missing mcp stdin");
        let stdout = child.stdout.take().expect("missing mcp stdout");
        let (sender, responses) = mpsc::channel();

        let reader = thread::spawn(move || {
            let mut lines = BufReader::new(stdout).lines();
            while let Some(Ok(line)) = lines.next() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        Self {
            child,
            stdin,
            responses,
            reader: Some(reader),
        }
    }

    fn initialize(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "lspyx-test",
                    "version": "0.0.0",
                },
            },
        }));
        let response = self.read_response();
        assert!(response.get("result").is_some(), "{response}");

        self.send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {},
        }));
    }

    fn send(&mut self, message: Value) {
        serde_json::to_writer(&mut self.stdin, &message).expect("failed to write mcp message");
        self.stdin
            .write_all(b"\n")
            .expect("failed to terminate mcp message");
        self.stdin.flush().expect("failed to flush mcp message");
    }

    fn read_response(&self) -> Value {
        let line = self
            .responses
            .recv_timeout(Duration::from_secs(5))
            .expect("mcp server did not respond");
        serde_json::from_str(&line).expect("mcp server returned invalid JSON")
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn mcp_lists_lspyx_explore_tool() {
    let mut server = McpServer::start();
    server.initialize();

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {},
    }));

    let response = server.read_response();
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list did not return tools");
    let tool_names = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool missing name"))
        .collect::<Vec<_>>();

    assert_eq!(tool_names, vec!["lspyx_explore"]);
}

#[test]
fn mcp_exposes_explore_control_schema() {
    let mut server = McpServer::start();
    server.initialize();

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {},
    }));

    let response = server.read_response();
    let input_schema = &response["result"]["tools"][0]["inputSchema"];
    let properties = input_schema["properties"]
        .as_object()
        .expect("input schema missing properties");

    for field in ["limit", "kind", "depth", "full"] {
        assert!(
            properties.contains_key(field),
            "input schema missing {field}"
        );
    }
}

#[test]
fn mcp_routes_lspyx_explore_invalid_params() {
    let mut server = McpServer::start();
    server.initialize();

    server.send(json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "lspyx_explore",
            "arguments": {
                "workspace": "/tmp",
            },
        },
    }));

    let response = server.read_response();
    let error = response.get("error").expect("tools/call should fail");
    let message = error["message"]
        .as_str()
        .expect("tools/call error missing message");

    assert!(
        message.contains("query is required when file is omitted"),
        "unexpected tools/call error: {response}"
    );
    assert_ne!(message, "tools/call");
}
