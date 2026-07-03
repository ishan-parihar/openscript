use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::io::{stdin, stdout};
use tokio::sync::Mutex;

use crate::error::ToolError;
use crate::tools::{route_tool, tool_definitions};

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonRpcMessage {
    Request {
        #[allow(dead_code)]
        jsonrpc: String,
        id: serde_json::Value,
        method: String,
        params: Option<serde_json::Value>,
    },
    Notification {
        #[allow(dead_code)]
        jsonrpc: String,
        method: String,
        #[allow(dead_code)]
        params: Option<serde_json::Value>,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum JsonRpcResponse {
    Result {
        jsonrpc: String,
        id: serde_json::Value,
        result: serde_json::Value,
    },
    Error {
        jsonrpc: String,
        id: serde_json::Value,
        error: JsonRpcError,
    },
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// Progress notification support
// ---------------------------------------------------------------------------

/// Global progress writer — shared across tool handlers so they can report
/// progress during long-running operations (transcription, rendering, etc.).
/// This prevents MCP client timeouts by resetting the client's timer.
pub struct ProgressWriter {
    stdout: Arc<Mutex<tokio::io::Stdout>>,
}

impl ProgressWriter {
    pub fn new() -> Self {
        Self {
            stdout: Arc::new(Mutex::new(stdout())),
        }
    }

    /// Send a progress notification to the client.
    /// The MCP protocol uses `notifications/progress` with a progress token,
    /// progress value, and optional total.
    pub async fn report_progress(
        &self,
        progress_token: &str,
        progress: f64,
        total: Option<f64>,
        message: Option<&str>,
    ) -> Result<(), std::io::Error> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {
                "progressToken": progress_token,
                "progress": progress,
                "total": total.unwrap_or(100.0),
                "message": message.unwrap_or(""),
            }
        });

        let json = serde_json::to_string(&notification)?;
        let msg = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        let mut out = self.stdout.lock().await;
        out.write_all(msg.as_bytes()).await?;
        out.flush().await?;
        Ok(())
    }
}

// Global progress writer — set once at startup
static PROGRESS_WRITER: OnceLock<Arc<ProgressWriter>> = OnceLock::new();

// Global progress token — updated per request (protected by Mutex)
static CURRENT_PROGRESS_TOKEN: OnceLock<std::sync::Mutex<String>> = OnceLock::new();

/// Set the progress token for the current tool call.
pub fn set_progress_token(token: String) {
    CURRENT_PROGRESS_TOKEN
        .get_or_init(|| std::sync::Mutex::new(String::new()))
        .lock()
        .unwrap()
        .clone_from(&token);
}

pub fn get_progress_token() -> Option<String> {
    CURRENT_PROGRESS_TOKEN.get().and_then(|m| {
        let s = m.lock().unwrap();
        if s.is_empty() { None } else { Some(s.clone()) }
    })
}

/// Report progress for the current tool call.
pub async fn report_progress(progress: f64, total: f64, message: &str) -> Result<(), std::io::Error> {
    if let Some(token) = get_progress_token() {
        if let Some(pw) = PROGRESS_WRITER.get() {
            pw.report_progress(&token, progress, Some(total), Some(message)).await
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

pub fn set_progress_writer(pw: Arc<ProgressWriter>) {
    PROGRESS_WRITER.get_or_init(|| pw);
}

// ---------------------------------------------------------------------------
// MCP handlers
// ---------------------------------------------------------------------------

fn handle_initialize() -> Result<serde_json::Value, ToolError> {
    Ok(serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {
                "listChanged": false
            },
            "logging": {}
        },
        "serverInfo": {
            "name": "openscript-rs",
            "version": "0.2.0"
        }
    }))
}

fn handle_tools_list() -> Result<serde_json::Value, ToolError> {
    let tools = tool_definitions();
    Ok(serde_json::json!({
        "tools": tools,
    }))
}

async fn handle_tools_call(
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, ToolError> {
    let params = params.ok_or_else(|| ToolError::MissingArg("params".to_string()))?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::MissingArg("name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Extract progress token if present (from _meta.progressToken)
    if let Some(meta) = params.get("_meta") {
        if let Some(token) = meta.get("progressToken").and_then(|v| v.as_str()) {
            set_progress_token(token.to_string());
        }
    }

    route_tool(name, args).await
}

// ---------------------------------------------------------------------------
// Message reader — supports both Content-Length framing and line-delimited JSON
// ---------------------------------------------------------------------------

/// Reads a single JSON-RPC message from stdin.
/// Supports both:
/// - Content-Length framing (MCP spec): `Content-Length: N\r\n\r\n{json}`
/// - Line-delimited JSON (TypeScript SDK): `{json}\n`
/// Returns None on EOF.
async fn read_message(reader: &mut BufReader<tokio::io::Stdin>) -> Option<Result<(String, bool), std::io::Error>> {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => return None,
            Ok(_) => {}
            Err(e) => return Some(Err(e)),
        }

        let trimmed = line.trim().to_string();

        if trimmed.starts_with("Content-Length: ") {
            if let Some(rest) = trimmed.strip_prefix("Content-Length: ") {
                if let Ok(len) = rest.parse::<usize>() {
                    loop {
                        let mut header = String::new();
                        match reader.read_line(&mut header).await {
                            Ok(0) => return None,
                            Ok(_) => {}
                            Err(e) => return Some(Err(e)),
                        }
                        if header.trim().is_empty() {
                            break;
                        }
                    }

                    let mut buf = vec![0u8; len];
                    let mut total_read = 0;
                    while total_read < len {
                        match reader.read(&mut buf[total_read..]).await {
                            Ok(0) => break,
                            Ok(n) => total_read += n,
                            Err(e) => return Some(Err(e)),
                        }
                    }

                    return match String::from_utf8(buf) {
                        Ok(s) => Some(Ok((s, true))),
                        Err(e) => Some(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Invalid UTF-8 in message body: {}", e),
                        ))),
                    };
                }
            }
        }

        if !trimmed.is_empty() {
            return Some(Ok((trimmed, false)));
        }
    }
}

// ---------------------------------------------------------------------------
// Response writer — adapts to client's framing style
// ---------------------------------------------------------------------------

async fn write_response(
    stdout: &mut tokio::io::Stdout,
    response: &JsonRpcResponse,
    use_content_length: bool,
) -> Result<(), std::io::Error> {
    let json = serde_json::to_string(response)?;
    if use_content_length {
        let msg = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        stdout.write_all(msg.as_bytes()).await?;
    } else {
        let msg = format!("{}\n", json);
        stdout.write_all(msg.as_bytes()).await?;
    }
    stdout.flush().await?;
    Ok(())
}

fn make_result_response(id: serde_json::Value, val: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse::Result {
        jsonrpc: "2.0".into(),
        id,
        result: val,
    }
}

fn make_tool_call_response(id: serde_json::Value, val: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse::Result {
        jsonrpc: "2.0".into(),
        id,
        result: serde_json::json!({
            "content": [{"type": "text", "text": serde_json::to_string(&val).unwrap_or_default()}]
        }),
    }
}

fn make_error_response(id: serde_json::Value, err: &ToolError) -> JsonRpcResponse {
    JsonRpcResponse::Error {
        jsonrpc: "2.0".into(),
        id,
        error: JsonRpcError {
            code: err.json_rpc_code(),
            message: err.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Main server loop
// ---------------------------------------------------------------------------

pub async fn run() -> Result<(), std::io::Error> {
    let stdin_reader = BufReader::new(stdin());
    let progress_writer = Arc::new(ProgressWriter::new());

    set_progress_writer(progress_writer.clone());

    let mut stdin_reader = stdin_reader;
    let mut stdout_writer = stdout();

    loop {
        let (message, msg_use_cl) = match read_message(&mut stdin_reader).await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => {
                eprintln!("stdin read error: {}", e);
                break;
            }
            None => break,
        };

        let use_content_length = msg_use_cl;

        let msg: JsonRpcMessage = match serde_json::from_str(&message) {
            Ok(m) => m,
            Err(e) => {
                let err_resp = JsonRpcResponse::Error {
                    jsonrpc: "2.0".into(),
                    id: serde_json::Value::Null,
                    error: JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                    },
                };
                write_response(&mut stdout_writer, &err_resp, use_content_length).await?;
                continue;
            }
        };

        match msg {
            JsonRpcMessage::Request {
                id,
                method,
                params,
                ..
            } => {
                let result = match method.as_str() {
                    "initialize" => handle_initialize(),
                    "tools/list" => handle_tools_list(),
                    "tools/call" => handle_tools_call(params).await,
                    "ping" => Ok(serde_json::json!({})),
                    _ => Err(ToolError::MethodNotFound(method.clone())),
                };

                let response = match result {
                    Ok(val) => {
                        if method == "tools/call" {
                            make_tool_call_response(id, val)
                        } else {
                            make_result_response(id, val)
                        }
                    }
                    Err(e) => make_error_response(id, &e),
                };

                write_response(&mut stdout_writer, &response, use_content_length).await?;
            }
            JsonRpcMessage::Notification { method, .. } => {
                if method == "notifications/initialized" || method == "initialized" {
                    tracing::info!("client initialized");
                }
            }
        }
    }

    Ok(())
}
