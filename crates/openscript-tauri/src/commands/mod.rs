// Command modules.
// `invoke_tool` is the generic MCP dispatcher (see invoke_tool.rs docs).
// The other modules are stateful wrappers for tools that need AppState.
pub mod assets;
pub mod invoke_tool;
pub mod project;
pub mod render;
pub mod timeline;
pub mod transcript;
pub mod tts;
