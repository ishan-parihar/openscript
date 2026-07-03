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
            "logging": {},
            "prompts": {
                "listChanged": false
            },
            "resources": {
                "listChanged": false,
                "subscribe": false
            },
            "experimental": {}
        },
        "serverInfo": {
            "name": "openscript-rs",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "OpenScript MCP server — 49 tools for AI-directed video editing. Use realize.timeline for one-call pipeline (raw video -> reel), or realize.brief + realize.direct for two-step control. HyperFrames (hf.*) tools for HTML+GSAP compositions; composition.render as the unified dispatcher. TTS routing: profiles with provider='kokoro' use native Kokoro, others use the faster-qwen3-tts sidecar."
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

    // Extract progress token if present (from _meta.progressToken).
    // Accept both string and integer tokens per MCP spec.
    if let Some(meta) = params.get("_meta") {
        if let Some(token_val) = meta.get("progressToken") {
            let token_str = match token_val {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            };
            if let Some(token) = token_str {
                set_progress_token(token);
            }
        }
    }

    let result = route_tool(name, args).await;

    // Clear the progress token after the tool call so subsequent requests
    // don't accidentally use the previous request's token.
    set_progress_token(String::new());

    result
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

        // Content-Length matching is case-insensitive per HTTP/RPC spec
        let lower = trimmed.to_lowercase();
        if lower.starts_with("content-length: ") {
            if let Some(rest) = lower.strip_prefix("content-length: ") {
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

/// Build a tool-call error response per MCP spec: result.content[0].text + isError:true.
/// Used for tool EXECUTION failures (sidecar down, file not found, ffmpeg crash).
fn make_tool_call_error_response(id: serde_json::Value, err: &ToolError) -> JsonRpcResponse {
    JsonRpcResponse::Result {
        jsonrpc: "2.0".into(),
        id,
        result: serde_json::json!({
            "content": [{"type": "text", "text": serde_json::to_string(&serde_json::json!({
                "error": err.to_string(),
                "error_type": format!("{:?}", err).split('(').next().unwrap_or("Unknown"),
            })).unwrap_or_default()}],
            "isError": true
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

/// Returns true if the error is a protocol-level error (should be JSON-RPC error)
/// vs an execution-level error (should be result.isError:true per MCP spec).
fn is_protocol_error(err: &ToolError) -> bool {
    matches!(
        err,
        ToolError::UnknownTool(_)
            | ToolError::MethodNotFound(_)
            | ToolError::MissingArg(_)
            | ToolError::InvalidArg(_)
            | ToolError::Json(_)
    )
}

// ---------------------------------------------------------------------------
// Main server loop — concurrent with cancellation support
// ---------------------------------------------------------------------------

/// Track in-flight tool call tasks for cancellation.
/// Maps request ID → AbortHandle.
type TaskMap = Arc<Mutex<std::collections::HashMap<serde_json::Value, tokio::task::AbortHandle>>>;

pub async fn run() -> Result<(), std::io::Error> {
    let stdin_reader = BufReader::new(stdin());
    let progress_writer = Arc::new(ProgressWriter::new());

    set_progress_writer(progress_writer.clone());

    let mut stdin_reader = stdin_reader;

    // mpsc channel for responses — single writer task drains this and writes to stdout
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(JsonRpcResponse, bool)>(128);

    // Spawn the stdout writer task
    let writer_task = tokio::spawn(async move {
        let mut stdout_writer = stdout();
        while let Some((response, use_content_length)) = rx.recv().await {
            if write_response(&mut stdout_writer, &response, use_content_length).await.is_err() {
                break;
            }
        }
    });

    // Track in-flight tool calls for cancellation
    let in_flight: TaskMap = Arc::new(Mutex::new(std::collections::HashMap::new()));

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
                let _ = tx.send((err_resp, use_content_length)).await;
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
                // Fast-path: initialize, tools/list, ping are synchronous (no spawn)
                let is_fast = matches!(method.as_str(), "initialize" | "tools/list" | "ping");

                if is_fast {
                    let result = match method.as_str() {
                        "initialize" => handle_initialize(),
                        "tools/list" => handle_tools_list(),
                        "ping" => Ok(serde_json::json!({})),
                        _ => unreachable!(),
                    };
                    let response = match result {
                        Ok(val) => make_result_response(id, val),
                        Err(e) => make_error_response(id, &e),
                    };
                    let _ = tx.send((response, use_content_length)).await;
                } else if method == "tools/call" {
                    // Spawn tool call as a concurrent task
                    let tx_clone = tx.clone();
                    let ucl = use_content_length;
                    let in_flight_clone = in_flight.clone();
                    let id_for_task = id.clone();

                    let task = tokio::spawn(async move {
                        let result = handle_tools_call(params).await;

                        // Remove from in-flight map
                        in_flight_clone.lock().await.remove(&id_for_task);

                        let response = match result {
                            Ok(val) => make_tool_call_response(id_for_task, val),
                            Err(e) => {
                                if !is_protocol_error(&e) {
                                    make_tool_call_error_response(id_for_task, &e)
                                } else {
                                    make_error_response(id_for_task, &e)
                                }
                            }
                        };
                        let _ = tx_clone.send((response, ucl)).await;
                    });

                    // Track for cancellation
                    in_flight.lock().await.insert(id.clone(), task.abort_handle());
                } else {
                    let response = make_error_response(id, &ToolError::MethodNotFound(method.clone()));
                    let _ = tx.send((response, use_content_length)).await;
                }
            }
            JsonRpcMessage::Notification { method, params, .. } => {
                if method == "notifications/initialized" || method == "initialized" {
                    tracing::info!("client initialized");
                } else if method == "notifications/cancelled" {
                    // Cancel the in-flight task for the given request ID
                    if let Some(id) = params
                        .as_ref()
                        .and_then(|p| p.get("requestId"))
                        .cloned()
                    {
                        if let Some(handle) = in_flight.lock().await.remove(&id) {
                            handle.abort();
                            tracing::info!("Cancelled request {:?}", id);
                        }
                    }
                }
            }
        }
    }

    // Close the channel and wait for the writer task to finish
    drop(tx);
    let _ = writer_task.await;

    Ok(())
}
