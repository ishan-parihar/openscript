// Command modules.
// `invoke_tool` is the generic MCP dispatcher (see invoke_tool.rs docs).
// The other modules are stateful wrappers for tools that need AppState.
pub mod project;
pub mod transcript;
pub mod timeline;
pub mod render;
pub mod assets;
pub mod tts;
pub mod invoke_tool;
