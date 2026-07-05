//! Generic MCP tool dispatch from the Tauri frontend.
//!
//! This is the keystone of the "desktop as MCP client of itself" architecture
//! (see AGENTS.md §5). Instead of maintaining a parallel `commands/*.rs` layer
//! that duplicates MCP tool logic, the frontend calls a SINGLE Tauri command —
//! `invoke_tool(name, args)` — which is a thin pass-through to
//! `openscript_mcp::tools::route_tool()`.
//!
//! This closes the 43/68 wiring gap by construction: every MCP tool is
//! automatically reachable from the frontend. The existing typed Tauri commands
//! (add_segment, broll_fetch, etc.) remain for backward compatibility and for
//! the stateful operations that need AppState (undo, autosave) — but new tools
//! and the AI command palette route exclusively through `invoke_tool`.
//!
//! ## State management
//!
//! `invoke_tool` is a STATELESS pass-through. It does NOT touch AppState, does
//! NOT record undo snapshots, does NOT autosave. Tools that need state
//! management (timeline mutations, project saves) should either:
//!   (a) use the existing typed Tauri commands (which DO manage state), OR
//!   (b) be wrapped in a new stateful command if they need undo/autosave.
//!
//! Read-only tools (system.capabilities, help.tool, sfx.search, hf.classify,
//! script.parse, etc.) are safe to call via `invoke_tool` directly.
//!
//! ## Error mapping
//!
//! `ToolError` is converted to a string via `Display` and returned as `Err`.
//! The frontend's `invokeTool<T>()` wrapper re-throws this as a TypeScript
//! error, which the stores catch and surface as a toast.

use openscript_mcp::tools::route_tool;
use serde_json::Value;
use tauri::State;

use crate::state::AppState;

/// Dispatch any MCP tool by name. The frontend calls this via
/// `invoke("invoke_tool", { name, args })`.
///
/// Returns the tool's JSON result on success, or the tool's error message
/// (with inline log context where applicable — see P0-2 fix) on failure.
#[tauri::command]
pub async fn invoke_tool(
    _state: State<'_, AppState>,
    name: String,
    args: Value,
) -> Result<Value, String> {
    route_tool(&name, args).await.map_err(|e| format!("{}", e))
}

/// List all registered MCP tools (name + description + inputSchema).
/// The frontend uses this to render the command palette and to discover
/// available tools at runtime.
#[tauri::command]
pub async fn list_mcp_tools(_state: State<'_, AppState>) -> Result<Value, String> {
    Ok(openscript_mcp::tools::tool_definitions())
}

/// Get a single tool's definition (name + description + inputSchema).
/// Returns `null` if the tool name is not registered.
#[tauri::command]
pub async fn get_mcp_tool(_state: State<'_, AppState>, name: String) -> Result<Value, String> {
    let tools = openscript_mcp::tools::tool_definitions();
    if let Some(arr) = tools.as_array() {
        for tool in arr {
            if tool.get("name").and_then(|v| v.as_str()) == Some(&name) {
                return Ok(tool.clone());
            }
        }
    }
    Ok(Value::Null)
}
